use codestory_contracts::api::{
    AffectedAnalysisDto, AffectedAnalysisRequest, AffectedChangeKindDto, AffectedChangeRecordDto,
    AffectedMatchedFileDto, AffectedRouteDto, AffectedSymbolDto, AffectedTestFileDto,
    AffectedUnmatchedPathDto, AgentAnswerDto, AgentAskRequest, AgentHybridWeightsDto,
    AgentPacketDto, AgentPacketRequestDto, ApiError, AppEventPayload, BookmarkCategoryDto,
    BookmarkDto, CreateBookmarkCategoryRequest, CreateBookmarkRequest, EdgeId, EdgeKind,
    EdgeOccurrencesRequest, EmbeddingProfileContractDto, FrameworkRouteCoverageDto, GraphEdgeDto,
    GraphNodeDto, GraphRequest, GraphResponse, GroundingBudgetDto, GroundingCoverageBucketDto,
    GroundingFileDigestDto, GroundingSnapshotDto, GroundingSymbolDigestDto, IndexDryRunDto,
    IndexFreshnessChangeKindDto, IndexFreshnessDto, IndexFreshnessSampleDto,
    IndexFreshnessStatusDto, IndexMode, IndexedFileDto, IndexedFileLanguageCountDto,
    IndexedFileRoleDto, IndexedFilesDto, IndexedFilesRequest, IndexedFilesSummaryDto,
    IndexingPhaseTimings, ListChildrenSymbolsRequest, ListRootSymbolsRequest, MemberAccess,
    NodeDetailsDto, NodeDetailsRequest, NodeId, NodeKind, NodeOccurrencesRequest,
    OpenContainingFolderRequest, OpenDefinitionRequest, OpenProjectRequest, ProjectSummary,
    ReadFileTextRequest, ReadFileTextResponse, RepoTextScanStatsDto, RetrievalFallbackReasonDto,
    RetrievalModeDto, RetrievalScoreBreakdownDto, RetrievalStateDto, RouteEndpointHandlerDto,
    RouteEndpointKindDto, RouteEndpointMetadataDto, SearchHit, SearchHitOrigin,
    SearchHybridLimitsDto, SearchMatchQualityDto, SearchPlanAnchorGroupDto,
    SearchPlanBridgeConfidenceDto, SearchPlanBridgeDto, SearchPlanBridgeEvidenceKindDto,
    SearchPlanBridgeStatusDto, SearchPlanCandidateWindowDto, SearchPlanChannelDto,
    SearchPlanDroppedTermDto, SearchPlanDto, SearchPlanNextActionDto, SearchPlanPromotionStatusDto,
    SearchPlanRejectedHitDto, SearchPlanSubqueryDto, SearchPlanTermsDto, SearchQueryAssessmentDto,
    SearchRepoTextMode, SearchRequest, SearchResultsDto, SemanticModeDto, SnippetContextDto,
    SourceOccurrenceDto, StartIndexingRequest, StorageStatsDto, StoredSemanticDocsContractDto,
    SummaryGenerationDto, SymbolContextDto, SymbolSummaryDto, SystemActionResponse, TrailConfigDto,
    TrailContextDto, TrailFilterOptionsDto, UpdateBookmarkCategoryRequest, UpdateBookmarkRequest,
    WorkspaceMemberIndexDto, WriteFileResponse, WriteFileTextRequest,
};
use codestory_contracts::events::{Event, EventBus};
use codestory_contracts::graph::{AccessKind, Edge as GraphEdge, Node as GraphNode};
use codestory_contracts::language_support::{
    LanguageSupportProfile, language_support_profile_for_ext,
    language_support_profile_for_language_name,
};
use codestory_indexer::IncrementalIndexingStats;
use codestory_indexer::WorkspaceIndexer as V2WorkspaceIndexer;
use codestory_store::{
    FileInfo, GroundingEdgeKindCount, GroundingNodeRecord, LlmSymbolDoc, LlmSymbolDocReuseMetadata,
    LlmSymbolDocStats, SearchSymbolProjection, SnapshotStore, Store, SymbolSearchDoc,
    SymbolSummaryRecord,
};
use codestory_workspace::{
    IndexedFileRecord, RefreshExecutionPlan, RefreshInputs, Workspace, WorkspaceInventory,
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use rayon::prelude::*;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod agent;
pub use agent::packet_step_trace_json;
mod browser;
pub mod graph_analysis;
mod graph_builders;
mod graph_canonical;
mod grounding;
mod mermaid;
mod path_resolution;
mod query_language;
mod repository_identity;
mod search;
mod search_runtime;
mod semantic_doc_text;
mod services;
mod support;
mod symbol_query;
mod system_actions;
mod trail_story;

pub use browser::{BrowserQueryItem, ReadOnlyBrowserService};
pub use codestory_contracts as contracts;
pub(crate) use mermaid::{fallback_mermaid, mermaid_flowchart, mermaid_gantt, mermaid_sequence};
pub use query_language::{GraphQueryParseError, parse_graph_query};
pub use repository_identity::{
    REPOSITORY_IDENTITY_SCHEMA_VERSION, RepositoryIdentityReport, inspect_repository_identity,
};
pub(crate) use search_runtime::SearchEngine;

#[cfg(test)]
static PROCESS_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn process_env_test_lock() -> std::sync::MutexGuard<'static, ()> {
    PROCESS_ENV_TEST_LOCK.lock().expect("process env test lock")
}
pub use search_runtime::*;
use semantic_doc_text::{
    runtime_concept_phrases, semantic_doc_language_from_path, semantic_path_aliases,
    semantic_symbol_aliases, semantic_symbol_role_aliases,
};
pub use services::{
    AgentService, BookmarkService, GroundingService, IndexService, ProjectService, SearchService,
    TrailService,
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

    if canonical
        .provenance
        .iter()
        .any(|entry| entry == "extraction:ast_indexed")
    {
        return 0.025;
    }
    if canonical
        .provenance
        .iter()
        .any(|entry| entry == "extraction:text_only")
    {
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
struct AffectedGraphEvidence {
    distance: u32,
    reason: String,
    confidence: String,
}

fn normalized_affected_change_records(
    req: &AffectedAnalysisRequest,
) -> Vec<AffectedChangeRecordDto> {
    if !req.change_records.is_empty() {
        return req.change_records.clone();
    }
    req.changed_paths
        .iter()
        .filter(|path| !path.trim().is_empty())
        .map(|path| AffectedChangeRecordDto {
            path: path.trim().to_string(),
            kind: AffectedChangeKindDto::Unknown,
            status: "path".to_string(),
            previous_path: None,
        })
        .collect()
}

fn affected_change_record_keys(record: &AffectedChangeRecordDto) -> Vec<String> {
    let mut keys = Vec::new();
    let path = normalize_path_key(&record.path);
    if !path.is_empty() {
        keys.push(path);
    }
    if let Some(previous_path) = record.previous_path.as_deref() {
        let previous = normalize_path_key(previous_path);
        if !previous.is_empty() {
            keys.push(previous);
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

fn affected_unmatched_reason(record: &AffectedChangeRecordDto) -> String {
    match record.kind {
        AffectedChangeKindDto::Deleted => {
            "deleted path did not match any indexed file; the index may already be stale or the path was never indexed"
                .to_string()
        }
        AffectedChangeKindDto::Renamed | AffectedChangeKindDto::Copied => {
            "renamed/copied path did not match current or previous indexed file path; reindex if the file moved"
                .to_string()
        }
        AffectedChangeKindDto::Untracked => {
            "untracked path is not in the index yet; run index --refresh incremental before graph traversal"
                .to_string()
        }
        _ => "path did not match any indexed file; reindex or pass repo-relative paths".to_string(),
    }
}

fn affected_edge_kind_label(kind: codestory_contracts::graph::EdgeKind) -> &'static str {
    match kind {
        codestory_contracts::graph::EdgeKind::MEMBER => "member",
        codestory_contracts::graph::EdgeKind::TYPE_USAGE => "type_usage",
        codestory_contracts::graph::EdgeKind::USAGE => "usage",
        codestory_contracts::graph::EdgeKind::CALL => "call",
        codestory_contracts::graph::EdgeKind::INHERITANCE => "inheritance",
        codestory_contracts::graph::EdgeKind::OVERRIDE => "override",
        codestory_contracts::graph::EdgeKind::TYPE_ARGUMENT => "type_argument",
        codestory_contracts::graph::EdgeKind::TEMPLATE_SPECIALIZATION => "template_specialization",
        codestory_contracts::graph::EdgeKind::INCLUDE => "include",
        codestory_contracts::graph::EdgeKind::IMPORT => "import",
        codestory_contracts::graph::EdgeKind::MACRO_USAGE => "macro_usage",
        codestory_contracts::graph::EdgeKind::ANNOTATION_USAGE => "annotation_usage",
        codestory_contracts::graph::EdgeKind::UNKNOWN => "unknown",
    }
}

fn affected_edge_confidence(edge: &codestory_contracts::graph::Edge) -> String {
    edge_certainty_label(edge.kind, edge.certainty, edge.confidence)
        .unwrap_or_else(|| "graph".to_string())
}

fn affected_dependent_evidence(
    distance: u32,
    edge: Option<&codestory_contracts::graph::Edge>,
    target_label: String,
) -> AffectedGraphEvidence {
    let (reason, confidence) = edge
        .map(|edge| {
            (
                format!(
                    "dependent reaches changed code via {} edge to {}",
                    affected_edge_kind_label(edge.kind),
                    target_label
                ),
                affected_edge_confidence(edge),
            )
        })
        .unwrap_or_else(|| {
            (
                format!("dependent reaches changed code through graph walk to {target_label}"),
                "graph".to_string(),
            )
        });
    AffectedGraphEvidence {
        distance,
        reason,
        confidence,
    }
}

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
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_when_handler_name_resolves",
        unsupported_patterns: &[
            "router composition and middleware arrays may need source review",
            "handler linking is name-based unless graph resolution confirms the target",
        ],
        known_gaps: &["mounted app prefixes are not globally propagated"],
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
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_when_handler_name_resolves",
        unsupported_patterns: &["plugin prefixes and schema-only route declarations are partial"],
        known_gaps: &["register() prefix propagation is not modeled"],
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
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "decorator",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["router prefixes and dependency-driven routing are partial"],
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
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            controller: AppController::new(),
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
        ReadOnlyBrowserService::new(self.controller.clone())
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

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SemanticProjectionMode {
    PersistBackedDocs,
    SkipPersistence,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SemanticProjectionStats {
    reported: bool,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SearchStateBuildStats {
    search_projection_rebuild_ms: u32,
    search_symbol_index_ms: u32,
}

struct SearchStateBuildResult {
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
    timings.search_symbol_index_ms = Some(stats.search_stats.search_symbol_index_ms);
    timings.runtime_cache_publish_ms = stats.runtime_cache_publish_ms;
    apply_semantic_projection_stats(timings, stats.semantic_stats);
}

fn build_search_state(
    storage: &mut Storage,
    search_storage_path: Option<&Path>,
    nodes: Vec<codestory_contracts::graph::Node>,
    llm_refresh_file_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
    semantic_projection_mode: SemanticProjectionMode,
    hydrate_semantic_docs: bool,
) -> Result<SearchStateBuildResult, ApiError> {
    let projection_started = Instant::now();
    match llm_refresh_file_scope {
        Some(scope) => storage.rebuild_search_symbol_projection_for_file_scope(scope),
        None => storage.rebuild_search_symbol_projection_from_node_table(),
    }
    .map_err(|e| ApiError::internal(format!("Failed to rebuild search symbol projection: {e}")))?;
    let search_projection_rebuild_ms = clamp_u128_to_u32(projection_started.elapsed().as_millis());

    let search_index_started = Instant::now();
    let mut node_names = HashMap::new();
    let mut engine = SearchEngine::new(search_storage_path)
        .map_err(|e| ApiError::internal(format!("Failed to init search engine: {e}")))?;
    let mut search_nodes = Vec::with_capacity(nodes.len().min(SEARCH_NODE_BATCH_SIZE));
    for node in &nodes {
        let display_name = node_display_name(node);
        node_names.insert(node.id, display_name.clone());
        search_nodes.push((node.id, display_name));
        if search_nodes.len() >= SEARCH_NODE_BATCH_SIZE {
            engine
                .index_nodes(std::mem::take(&mut search_nodes))
                .map_err(|e| ApiError::internal(format!("Failed to index search nodes: {e}")))?;
        }
    }
    if !search_nodes.is_empty() {
        engine
            .index_nodes(search_nodes)
            .map_err(|e| ApiError::internal(format!("Failed to index search nodes: {e}")))?;
    }
    let search_symbol_index_ms = clamp_u128_to_u32(search_index_started.elapsed().as_millis());
    let search_stats = SearchStateBuildStats {
        search_projection_rebuild_ms,
        search_symbol_index_ms,
    };
    if semantic_projection_mode == SemanticProjectionMode::PersistBackedDocs {
        let semantic_stats = sync_llm_symbol_projection(
            storage,
            &nodes,
            &node_names,
            &mut engine,
            llm_refresh_file_scope,
            hydrate_semantic_docs,
        )?;
        Ok(SearchStateBuildResult {
            node_names,
            engine,
            search_stats,
            semantic_stats,
        })
    } else {
        tracing::debug!(
            "Skipping semantic doc persistence for transient build_search_state invocation"
        );
        engine.index_llm_symbol_docs(Vec::new());
        Ok(SearchStateBuildResult {
            node_names,
            engine,
            search_stats,
            semantic_stats: SemanticProjectionStats::default(),
        })
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

fn refresh_inputs_from_files(files: Vec<FileInfo>) -> RefreshInputs {
    let inventory = files
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                IndexedFileRecord {
                    file_id: file.id,
                    modification_time: file.modification_time,
                    indexed: file.indexed,
                },
            )
        })
        .collect::<Vec<_>>();
    let stored_files = files
        .into_iter()
        .map(|file| codestory_workspace::StoredFileState {
            id: file.id,
            path: file.path,
            modification_time: file.modification_time,
            indexed: file.indexed,
        });

    RefreshInputs {
        stored_files: stored_files.collect(),
        inventory: WorkspaceInventory::from_records(inventory),
    }
}

fn indexable_source_path(path: &Path) -> bool {
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

fn looks_like_openapi_source_path(path: &Path) -> bool {
    if !codestory_indexer::is_openapi_candidate_path(path) {
        return false;
    }
    let Ok(source) = std::fs::read_to_string(path) else {
        return true;
    };
    codestory_indexer::looks_like_openapi_schema(&source)
}

fn index_freshness_from_storage(
    root: &Path,
    workspace: &Workspace,
    storage: &Storage,
) -> IndexFreshnessDto {
    let started_at = Instant::now();
    let files = match storage.get_files() {
        Ok(files) => files,
        Err(error) => {
            return not_checked_index_freshness(
                format!("failed to read indexed file inventory: {error}"),
                0,
                started_at,
            );
        }
    };
    let indexed_file_count = clamp_usize_to_u32(files.len());
    if files.is_empty() {
        return not_checked_index_freshness(
            "no indexed file inventory is available yet",
            indexed_file_count,
            started_at,
        );
    }
    if files.len() > INDEX_FRESHNESS_INDEXED_FILE_CAP {
        return not_checked_index_freshness(
            format!(
                "indexed file inventory exceeds bounded freshness cap ({} > {})",
                files.len(),
                INDEX_FRESHNESS_INDEXED_FILE_CAP
            ),
            indexed_file_count,
            started_at,
        );
    }

    let removed_paths = files
        .iter()
        .map(|file| (file.id, file.path.clone()))
        .collect::<HashMap<_, _>>();
    let refresh_inputs = refresh_inputs_from_files(files);
    let plan = match workspace
        .build_execution_plan_bounded(&refresh_inputs, INDEX_FRESHNESS_CURRENT_FILE_CAP)
    {
        Ok(Some(plan)) => plan,
        Ok(None) => {
            return not_checked_index_freshness(
                format!(
                    "current workspace inventory exceeds bounded freshness cap (>{})",
                    INDEX_FRESHNESS_CURRENT_FILE_CAP
                ),
                indexed_file_count,
                started_at,
            );
        }
        Err(error) => {
            return not_checked_index_freshness(
                format!("failed to check workspace inventory: {error}"),
                indexed_file_count,
                started_at,
            );
        }
    };

    let mut changed_file_count = 0u32;
    let mut new_file_count = 0u32;
    let mut samples = Vec::new();
    for path in &plan.files_to_index {
        let existing_indexed_file = plan.existing_file_ids.contains_key(path);
        if !existing_indexed_file && !indexable_source_path(path) {
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

    let removed_file_count = clamp_usize_to_u32(plan.files_to_remove.len());
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

    let status = if changed_file_count == 0 && new_file_count == 0 && removed_file_count == 0 {
        IndexFreshnessStatusDto::Fresh
    } else {
        IndexFreshnessStatusDto::Stale
    };
    let checked_file_count = indexed_file_count
        .saturating_sub(removed_file_count)
        .saturating_add(new_file_count);

    IndexFreshnessDto {
        status,
        changed_file_count,
        new_file_count,
        removed_file_count,
        checked_file_count,
        indexed_file_count,
        duration_ms: clamp_u128_to_u32(started_at.elapsed().as_millis()),
        reason: None,
        samples,
    }
}

fn workspace_member_index_summaries(
    root: &Path,
    workspace: &Workspace,
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
    workspace: &Workspace,
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
        && let Ok(max_tokens) = std::env::var("CODESTORY_SUMMARY_MAX_TOKENS")
            .unwrap_or_default()
            .parse::<u32>()
    {
        object.insert("max_tokens".to_string(), serde_json::json!(max_tokens));
    }

    let body = serde_json::to_string(&request)
        .map_err(|e| ApiError::internal(format!("Failed to build summary request: {e}")))?;
    let mut request = ureq::post(endpoint)
        .timeout(summary_endpoint_timeout())
        .set("Content-Type", "application/json");
    if let Ok(api_key) = std::env::var("CODESTORY_SUMMARY_API_KEY")
        && !api_key.trim().is_empty()
    {
        request = request.set("Authorization", &format!("Bearer {}", api_key.trim()));
    }
    let response_body = request
        .send_string(&body)
        .map_err(summary_endpoint_http_error)?
        .into_string()
        .map_err(|e| ApiError::internal(format!("Summary endpoint response failed: {e}")))?;
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

fn summary_endpoint_timeout() -> Duration {
    let seconds = std::env::var("CODESTORY_SUMMARY_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| (1..=300).contains(seconds))
        .unwrap_or(30);
    Duration::from_secs(seconds)
}

fn summary_endpoint_http_error(error: ureq::Error) -> ApiError {
    match error {
        ureq::Error::Status(status, response) => {
            let body = response
                .into_string()
                .unwrap_or_else(|read_error| format!("failed to read error body: {read_error}"));
            ApiError::internal(format!(
                "Summary endpoint failed with status {status}: {}",
                truncate_error_body(&body)
            ))
        }
        ureq::Error::Transport(error) => {
            ApiError::internal(format!("Summary endpoint request failed: {error}"))
        }
    }
}

fn truncate_error_body(body: &str) -> String {
    const MAX_ERROR_BODY_CHARS: usize = 2_048;
    let mut truncated = body.chars().take(MAX_ERROR_BODY_CHARS).collect::<String>();
    if body.chars().count() > MAX_ERROR_BODY_CHARS {
        truncated.push_str("...");
    }
    truncated
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

const LLM_SYMBOL_DOC_SCHEMA_VERSION: u32 = 5;
const LLM_SYMBOL_DOC_VERSION_PREFIX: &str = "semantic_doc_version:";
const SEARCH_NODE_BATCH_SIZE: usize = 8_192;
const SEARCH_SYMBOL_PROJECTION_BATCH_SIZE: usize = 4_096;
const LLM_DOC_RELOAD_BATCH_SIZE: usize = 512;
const LLM_DOC_EMBED_BATCH_SIZE: usize = 128;
const LLM_DOC_EMBED_BATCH_SIZE_ENV: &str = "CODESTORY_LLM_DOC_EMBED_BATCH_SIZE";
const SEMANTIC_DOC_SCOPE_ENV: &str = "CODESTORY_SEMANTIC_DOC_SCOPE";
const SEMANTIC_DOC_ALIAS_MODE_ENV: &str = "CODESTORY_SEMANTIC_DOC_ALIAS_MODE";
const SEMANTIC_DOC_MAX_TOKENS_ENV: &str = "CODESTORY_SEMANTIC_DOC_MAX_TOKENS";
const SEMANTIC_DOC_DEFAULT_MAX_TOKENS: usize = 128;
const SEMANTIC_STREAM_PENDING_DOCS_ENV: &str = "CODESTORY_SEMANTIC_STREAM_PENDING_DOCS";
const SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV: &str =
    "CODESTORY_SEMANTIC_STREAM_SORT_WINDOW_BATCHES";
const SEMANTIC_STREAM_SORT_WINDOW_BATCHES: usize = 1;
const SEMANTIC_POLICY_VERSION: &str = "graph_first_v1";
const SYMBOL_SEARCH_DOC_PROVENANCE: &str = "extracted";
const DENSE_CENTRAL_LABEL_THRESHOLD: usize = 12;
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

fn search_index_storage_path(storage_path: &Path) -> PathBuf {
    let parent = storage_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = storage_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("codestory");
    parent.join(format!("{stem}.search"))
}

fn ensure_search_symbol_projection(storage: &mut Storage) -> Result<u32, ApiError> {
    let count = storage.get_search_symbol_projection_count().map_err(|e| {
        ApiError::internal(format!(
            "Failed to query search symbol projection count: {e}"
        ))
    })?;
    if count > 0 {
        return Ok(count);
    }

    storage
        .rebuild_search_symbol_projection_from_node_table()
        .map_err(|e| ApiError::internal(format!("Failed to rebuild search symbol projection: {e}")))
}

fn load_search_symbol_projection(
    storage: &Storage,
    batch_size: usize,
) -> Result<
    (
        HashMap<codestory_contracts::graph::NodeId, String>,
        Vec<SearchSymbolProjection>,
    ),
    ApiError,
> {
    let mut node_names = HashMap::new();
    let mut entries = Vec::new();
    let mut after_node_id = None;
    let batch_size = batch_size.max(1);
    loop {
        let batch = storage
            .get_search_symbol_projection_batch_after(after_node_id, batch_size)
            .map_err(|e| {
                ApiError::internal(format!("Failed to load search symbol projection: {e}"))
            })?;
        if batch.is_empty() {
            break;
        }
        after_node_id = batch.last().map(|entry| entry.node_id);
        for entry in batch {
            node_names.insert(entry.node_id, entry.display_name.clone());
            entries.push(entry);
        }
    }
    Ok((node_names, entries))
}

fn build_search_engine_from_projection(
    search_storage_path: &Path,
    projection: &[SearchSymbolProjection],
) -> Result<SearchEngine, ApiError> {
    index_projection_into_search_engine(SearchEngine::new(Some(search_storage_path)), projection)
}

fn rebuild_search_engine_from_projection(
    search_storage_path: &Path,
    projection: &[SearchSymbolProjection],
    existing: SearchEngine,
) -> Result<SearchEngine, ApiError> {
    index_projection_into_search_engine(
        SearchEngine::recreate_persisted_from_existing(search_storage_path, existing),
        projection,
    )
}

fn index_projection_into_search_engine(
    engine: anyhow::Result<SearchEngine>,
    projection: &[SearchSymbolProjection],
) -> Result<SearchEngine, ApiError> {
    let mut engine =
        engine.map_err(|e| ApiError::internal(format!("Failed to init search engine: {e}")))?;
    let mut search_nodes = Vec::with_capacity(projection.len().min(SEARCH_NODE_BATCH_SIZE));
    for entry in projection {
        search_nodes.push((entry.node_id, entry.display_name.clone()));
        if search_nodes.len() >= SEARCH_NODE_BATCH_SIZE {
            engine
                .index_nodes(std::mem::take(&mut search_nodes))
                .map_err(|e| ApiError::internal(format!("Failed to index search nodes: {e}")))?;
        }
    }
    if !search_nodes.is_empty() {
        engine
            .index_nodes(search_nodes)
            .map_err(|e| ApiError::internal(format!("Failed to index search nodes: {e}")))?;
    }
    Ok(engine)
}

fn load_persisted_search_state(
    storage: &mut Storage,
    storage_path: &Path,
) -> Result<
    (
        HashMap<codestory_contracts::graph::NodeId, String>,
        SearchEngine,
    ),
    ApiError,
> {
    ensure_search_symbol_projection(storage)?;
    let (node_names, projection) =
        load_search_symbol_projection(storage, SEARCH_SYMBOL_PROJECTION_BATCH_SIZE)?;
    let search_storage_path = search_index_storage_path(storage_path);

    let engine = if projection.is_empty() {
        build_search_engine_from_projection(search_storage_path.as_path(), &projection)?
    } else {
        let (mut engine, open_error) =
            SearchEngine::open_existing_or_recreate(search_storage_path.as_path())
                .map_err(|e| ApiError::internal(format!("Failed to init search engine: {e}")))?;
        if let Some(error) = open_error {
            tracing::warn!(
                "Failed to open persisted search index at {}: {}. Rebuilding from projection.",
                search_storage_path.display(),
                error
            );
            index_projection_into_search_engine(Ok(engine), &projection)?
        } else {
            engine.load_symbol_projection(
                projection
                    .iter()
                    .map(|entry| (entry.node_id, entry.display_name.clone())),
            );
            if engine.full_text_doc_count() != projection.len() {
                tracing::warn!(
                    "Persisted search index at {} has {} docs but projection has {}. Rebuilding from projection.",
                    search_storage_path.display(),
                    engine.full_text_doc_count(),
                    projection.len()
                );
                rebuild_search_engine_from_projection(
                    search_storage_path.as_path(),
                    &projection,
                    engine,
                )?
            } else {
                engine
            }
        }
    };
    Ok((node_names, engine))
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

fn llm_doc_embed_batch_size() -> usize {
    std::env::var(LLM_DOC_EMBED_BATCH_SIZE_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1, 2_048))
        .unwrap_or(LLM_DOC_EMBED_BATCH_SIZE)
}

fn retrieval_state_from_parts(
    semantic_doc_count: u32,
    embedding_model: Option<String>,
    embedding_runtime_available: bool,
    fallback_message: Option<String>,
    current_embedding: Option<EmbeddingProfileContractDto>,
    stored_embedding: Option<StoredSemanticDocsContractDto>,
    runtime_degraded: bool,
) -> RetrievalStateDto {
    let hybrid_configured = hybrid_retrieval_enabled();
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

fn retrieval_state_from_storage(storage: &Storage) -> Result<RetrievalStateDto, ApiError> {
    let stats = storage
        .get_llm_symbol_doc_stats()
        .map_err(|e| ApiError::internal(format!("Failed to query LLM symbol doc stats: {e}")))?;
    let probe = embedding_runtime_availability_from_env();
    let current_embedding = current_embedding_contract_from_env();
    let stored_embedding = stored_semantic_docs_contract_from_stats(&stats);
    Ok(retrieval_state_from_parts(
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
        probe.fallback_message,
        current_embedding,
        Some(stored_embedding),
        false,
    ))
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

fn semantic_doc_scope_from_env() -> SemanticDocScope {
    semantic_doc_scope_from_value(&std::env::var(SEMANTIC_DOC_SCOPE_ENV).unwrap_or_default())
}

fn semantic_doc_scope_from_value(value: &str) -> SemanticDocScope {
    match value.trim().to_ascii_lowercase().as_str() {
        "all" | "full" | "all-symbols" | "all_symbols" => SemanticDocScope::AllSymbols,
        _ => SemanticDocScope::DurableSymbols,
    }
}

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

fn semantic_doc_max_tokens_from_env() -> usize {
    std::env::var(SEMANTIC_DOC_MAX_TOKENS_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.clamp(16, 8_192))
        .unwrap_or(SEMANTIC_DOC_DEFAULT_MAX_TOKENS)
}

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

fn semantic_file_table_read_path_map(files: Vec<FileInfo>) -> HashMap<GraphNodeId, String> {
    let (_, read_paths) = semantic_file_table_path_maps(files);
    read_paths
}

#[derive(Default)]
struct SemanticDocGraphContext {
    child_labels: HashMap<GraphNodeId, Vec<String>>,
    referenced_labels: HashMap<GraphNodeId, Vec<String>>,
    edge_digests: HashMap<GraphNodeId, Vec<String>>,
    file_paths: HashMap<GraphNodeId, String>,
    file_read_paths: HashMap<GraphNodeId, String>,
}

impl SemanticDocGraphContext {
    fn build(
        storage: &Storage,
        semantic_nodes: &[&GraphNode],
        all_nodes: &[GraphNode],
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
        let files = storage
            .get_files()
            .map_err(|e| ApiError::internal(format!("Failed to load semantic doc files: {e}")))?;
        let file_table_paths = semantic_file_table_path_map(files.clone());
        let file_table_read_paths = semantic_file_table_read_path_map(files);

        let mut context = Self::default();
        for node in semantic_nodes {
            if let Some(file_id) = node.file_node_id
                && let Some(file_node) = nodes_by_id.get(&file_id)
            {
                let file_path = file_table_paths
                    .get(&file_id)
                    .cloned()
                    .unwrap_or_else(|| file_node.serialized_name.clone());
                context.file_paths.insert(node.id, file_path);
                let read_path = file_table_read_paths
                    .get(&file_id)
                    .cloned()
                    .unwrap_or_else(|| file_node.serialized_name.clone());
                context.file_read_paths.insert(node.id, read_path);
            }

            let edges = edges_by_node
                .get(&node.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            context.child_labels.insert(
                node.id,
                child_symbol_labels_from_edges(node, edges, &nodes_by_id, 6),
            );
            context.referenced_labels.insert(
                node.id,
                referenced_symbol_labels_from_edges(node, edges, &nodes_by_id, 6),
            );
            context
                .edge_digests
                .insert(node.id, edge_digest_for_edges(edges, 6));
        }

        Ok(context)
    }

    fn file_path_for_node(&self, node: &GraphNode) -> Option<&str> {
        self.file_paths.get(&node.id).map(String::as_str)
    }

    fn file_read_path_for_node(&self, node: &GraphNode) -> Option<&str> {
        self.file_read_paths
            .get(&node.id)
            .or_else(|| self.file_paths.get(&node.id))
            .map(String::as_str)
    }
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
    let mut file_paths = semantic_nodes
        .iter()
        .filter_map(|node| {
            let display_path = graph_context.file_path_for_node(node)?.to_string();
            let read_path = graph_context
                .file_read_path_for_node(node)
                .unwrap_or(display_path.as_str())
                .to_string();
            Some((display_path, read_path))
        })
        .collect::<HashMap<_, _>>()
        .into_iter()
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

fn referenced_symbol_labels_from_edges(
    node: &GraphNode,
    edges: &[GraphEdge],
    nodes_by_id: &HashMap<GraphNodeId, &GraphNode>,
    limit: usize,
) -> Vec<String> {
    let mut labels = Vec::new();

    for edge in edges {
        let (source, target) = edge.effective_endpoints();
        let other = if source == node.id {
            target
        } else if target == node.id {
            source
        } else {
            continue;
        };
        let Some(other_node) = nodes_by_id.get(&other) else {
            continue;
        };
        if !llm_indexable_kind(other_node.kind) {
            continue;
        }
        let label = node_display_name(other_node);
        if label.is_empty() || labels.contains(&label) {
            continue;
        }
        labels.push(label);
        if labels.len() >= limit {
            break;
        }
    }

    labels
}

fn child_symbol_labels_from_edges(
    node: &GraphNode,
    edges: &[GraphEdge],
    nodes_by_id: &HashMap<GraphNodeId, &GraphNode>,
    limit: usize,
) -> Vec<String> {
    edges
        .iter()
        .filter(|edge| edge.kind == codestory_contracts::graph::EdgeKind::MEMBER)
        .filter(|edge| edge.source == node.id)
        .filter_map(|edge| nodes_by_id.get(&edge.target).copied())
        .filter(|child| llm_indexable_kind(child.kind))
        .map(node_display_name)
        .filter(|label| !label.is_empty())
        .take(limit)
        .collect()
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

fn build_llm_symbol_doc_text(
    graph_context: &SemanticDocGraphContext,
    node: &GraphNode,
    display_name: &str,
    file_path: Option<&str>,
    file_text_cache: &HashMap<String, Option<String>>,
) -> String {
    let alias_mode = semantic_doc_alias_mode_from_env();
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
        let domain_aliases = runtime_concept_phrases(display_name, node.qualified_name.as_deref());
        if !domain_aliases.is_empty() {
            let _ = writeln!(out, "domain_aliases: {}", domain_aliases.join(", "));
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

    out = truncate_semantic_doc_text_to_token_budget(&out, semantic_doc_max_tokens_from_env());

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct SemanticVectorReuseContractKey {
    backend: String,
    profile: String,
    model_id: String,
    dimension: u32,
    doc_shape: String,
    semantic_policy_version: String,
}

impl SemanticVectorReuseContractKey {
    fn from_existing(existing_doc: &LlmSymbolDocReuseMetadata) -> Option<Self> {
        if existing_doc.doc_version != LLM_SYMBOL_DOC_SCHEMA_VERSION
            || existing_doc.embedding_dim == 0
        {
            return None;
        }
        Some(Self {
            backend: existing_doc.embedding_backend.clone()?,
            profile: existing_doc.embedding_profile.clone()?,
            model_id: existing_doc.embedding_model.clone(),
            dimension: existing_doc.embedding_dim,
            doc_shape: existing_doc.doc_shape.clone()?,
            semantic_policy_version: existing_doc.semantic_policy_version.clone()?,
        })
    }

    fn current(embedding_contract: &EmbeddingProfileContractDto, dimension: u32) -> Self {
        Self {
            backend: embedding_contract.backend.clone(),
            profile: embedding_contract.profile.clone(),
            model_id: embedding_contract.cache_key.clone(),
            dimension,
            doc_shape: embedding_contract.doc_shape.clone(),
            semantic_policy_version: SEMANTIC_POLICY_VERSION.to_string(),
        }
    }

    fn matches_current_without_known_dimension(
        &self,
        embedding_contract: &EmbeddingProfileContractDto,
    ) -> bool {
        self.backend.as_str() == embedding_contract.backend.as_str()
            && self.profile.as_str() == embedding_contract.profile.as_str()
            && self.model_id.as_str() == embedding_contract.cache_key.as_str()
            && self.dimension > 0
            && self.doc_shape.as_str() == embedding_contract.doc_shape.as_str()
            && self.semantic_policy_version.as_str() == SEMANTIC_POLICY_VERSION
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SemanticVectorReuseKey {
    contract: SemanticVectorReuseContractKey,
    doc_hash: String,
}

impl SemanticVectorReuseKey {
    fn from_existing(existing_doc: &LlmSymbolDocReuseMetadata) -> Option<Self> {
        if existing_doc.doc_hash.is_empty() {
            return None;
        }
        Some(Self {
            contract: SemanticVectorReuseContractKey::from_existing(existing_doc)?,
            doc_hash: existing_doc.doc_hash.clone(),
        })
    }

    fn current(
        doc_hash: &str,
        embedding_contract: &EmbeddingProfileContractDto,
        dimension: u32,
    ) -> Self {
        Self {
            contract: SemanticVectorReuseContractKey::current(embedding_contract, dimension),
            doc_hash: doc_hash.to_string(),
        }
    }

    fn matches_current_without_known_dimension(
        &self,
        doc_hash: &str,
        embedding_contract: &EmbeddingProfileContractDto,
    ) -> bool {
        self.doc_hash == doc_hash
            && self
                .contract
                .matches_current_without_known_dimension(embedding_contract)
    }
}

fn llm_symbol_doc_hash(doc_text: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    let alias_mode = semantic_doc_alias_mode_from_env();
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

fn llm_symbol_doc_contract_matches(
    existing_doc: &LlmSymbolDocReuseMetadata,
    embedding_contract: &EmbeddingProfileContractDto,
) -> bool {
    let Some(existing_key) = SemanticVectorReuseContractKey::from_existing(existing_doc) else {
        return false;
    };
    if let Some(dimension) = embedding_contract.dimension {
        return existing_key
            == SemanticVectorReuseContractKey::current(embedding_contract, dimension);
    }
    existing_key.matches_current_without_known_dimension(embedding_contract)
}

fn llm_symbol_doc_can_reuse(
    existing_doc: &LlmSymbolDocReuseMetadata,
    doc_hash: &str,
    embedding_contract: &EmbeddingProfileContractDto,
) -> bool {
    let Some(existing_key) = SemanticVectorReuseKey::from_existing(existing_doc) else {
        return false;
    };
    if let Some(dimension) = embedding_contract.dimension {
        return existing_key
            == SemanticVectorReuseKey::current(doc_hash, embedding_contract, dimension);
    }
    existing_key.matches_current_without_known_dimension(doc_hash, embedding_contract)
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

fn semantic_edge_count(edge_digests: &[String]) -> usize {
    edge_digests
        .iter()
        .filter_map(|digest| digest.rsplit_once('='))
        .filter_map(|(_, raw)| raw.parse::<usize>().ok())
        .sum()
}

fn dense_anchor_score(graph_context: &SemanticDocGraphContext, node_id: GraphNodeId) -> usize {
    let child_count = graph_context
        .child_labels
        .get(&node_id)
        .map(Vec::len)
        .unwrap_or(0);
    let related_count = graph_context
        .referenced_labels
        .get(&node_id)
        .map(Vec::len)
        .unwrap_or(0);
    let edge_count = graph_context
        .edge_digests
        .get(&node_id)
        .map(|digests| semantic_edge_count(digests))
        .unwrap_or(0);
    child_count
        .saturating_add(related_count)
        .saturating_add(edge_count)
}

fn dense_anchor_is_central(graph_context: &SemanticDocGraphContext, node_id: GraphNodeId) -> bool {
    let label_count = graph_context
        .child_labels
        .get(&node_id)
        .map(Vec::len)
        .unwrap_or(0)
        .saturating_add(
            graph_context
                .referenced_labels
                .get(&node_id)
                .map(Vec::len)
                .unwrap_or(0),
        );
    label_count >= DENSE_CENTRAL_LABEL_THRESHOLD
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

fn build_component_report_docs(
    graph_context: &SemanticDocGraphContext,
    semantic_nodes: &[&GraphNode],
    existing_docs: &HashMap<GraphNodeId, LlmSymbolDocReuseMetadata>,
    embedding_contract: Option<&EmbeddingProfileContractDto>,
    updated_at_epoch_ms: i64,
) -> Vec<BuiltLlmSymbolDoc> {
    let mut components = BTreeMap::<String, Vec<&GraphNode>>::new();
    for node in semantic_nodes {
        let file_path = graph_context.file_path_for_node(node);
        let Some(component_key) = semantic_component_key_for_path(file_path) else {
            continue;
        };
        components.entry(component_key).or_default().push(*node);
    }

    components
        .into_iter()
        .filter_map(|(component_key, mut component_nodes)| {
            component_nodes.sort_by(|left, right| {
                dense_anchor_score(graph_context, right.id)
                    .cmp(&dense_anchor_score(graph_context, left.id))
                    .then_with(|| node_display_name(left).cmp(&node_display_name(right)))
                    .then_with(|| left.id.0.cmp(&right.id.0))
            });
            let god_nodes = component_nodes
                .iter()
                .take(8)
                .map(|node| {
                    let file = graph_context.file_path_for_node(node).unwrap_or("");
                    format!(
                        "- {} kind={:?} file={} centrality={}",
                        node_display_name(node),
                        node.kind,
                        file,
                        dense_anchor_score(graph_context, node.id)
                    )
                })
                .collect::<Vec<_>>();
            if god_nodes.is_empty() {
                return None;
            }
            let mut files = component_nodes
                .iter()
                .filter_map(|node| graph_context.file_path_for_node(node))
                .map(str::to_string)
                .collect::<Vec<_>>();
            files.sort();
            files.dedup();
            files.truncate(12);
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
            let _ = writeln!(doc_text, "symbol_count: {}", component_nodes.len());
            let _ = writeln!(doc_text, "file_count: {}", files.len());
            if !files.is_empty() {
                let _ = writeln!(doc_text, "files: {}", files.join("; "));
            }
            let _ = writeln!(doc_text, "god_nodes:");
            for line in god_nodes {
                let _ = writeln!(doc_text, "{line}");
            }
            doc_text = truncate_semantic_doc_text_to_token_budget(
                &doc_text,
                semantic_doc_max_tokens_from_env(),
            );
            let doc_hash = llm_symbol_doc_hash(&doc_text);
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
            let pending = embedding_contract.map(|embedding_contract| {
                let dense_reason = DenseAnchorReason::ComponentReport;
                let reusable = existing_docs.get(&node_id).is_some_and(|existing_doc| {
                    llm_symbol_doc_can_reuse(existing_doc, &doc_hash, embedding_contract)
                        && existing_doc.dense_reason.as_deref() == Some(dense_reason.as_str())
                });
                (
                    PendingLlmSymbolDoc {
                        node_id,
                        file_node_id: None,
                        kind,
                        display_name,
                        qualified_name,
                        file_path: representative_file_path,
                        start_line: None,
                        doc_text,
                        doc_hash,
                        dense_reason,
                    },
                    reusable,
                )
            });
            let (pending, reusable) = pending
                .map(|(pending, reusable)| (Some(pending), reusable))
                .unwrap_or((None, false));
            Some(BuiltLlmSymbolDoc {
                symbol_doc,
                pending,
                reusable,
            })
        })
        .collect()
}

fn sort_pending_llm_symbol_docs_for_embedding_batches(docs: &mut [PendingLlmSymbolDoc]) {
    docs.sort_by(|left, right| {
        left.doc_text
            .len()
            .cmp(&right.doc_text.len())
            .then_with(|| left.node_id.0.cmp(&right.node_id.0))
    });
}

fn flush_pending_llm_symbol_docs(
    storage: &mut Storage,
    engine: &mut SearchEngine,
    batch: &[PendingLlmSymbolDoc],
    embedding_contract: &EmbeddingProfileContractDto,
    updated_at_epoch_ms: i64,
    stats: &mut SemanticProjectionStats,
) -> Result<(), ApiError> {
    if batch.is_empty() {
        return Ok(());
    }

    let payloads = batch
        .iter()
        .map(|doc| doc.doc_text.as_str())
        .collect::<Vec<_>>();
    let embedding_started = Instant::now();
    let embeddings = engine
        .embed_text_refs(&payloads)
        .map_err(|e| ApiError::internal(format!("Failed to embed symbol docs: {e:#}")))?;
    stats.embedding_ms = stats
        .embedding_ms
        .saturating_add(clamp_u128_to_u32(embedding_started.elapsed().as_millis()));

    let docs = batch
        .iter()
        .zip(embeddings)
        .map(|(doc, embedding)| LlmSymbolDoc {
            node_id: doc.node_id,
            file_node_id: doc.file_node_id,
            kind: doc.kind,
            display_name: doc.display_name.clone(),
            qualified_name: doc.qualified_name.clone(),
            file_path: doc.file_path.clone(),
            start_line: doc.start_line,
            doc_text: doc.doc_text.clone(),
            doc_version: LLM_SYMBOL_DOC_SCHEMA_VERSION,
            doc_hash: doc.doc_hash.clone(),
            embedding_profile: Some(embedding_contract.profile.clone()),
            embedding_model: embedding_contract.cache_key.clone(),
            embedding_backend: Some(embedding_contract.backend.clone()),
            embedding_dim: embedding.len() as u32,
            doc_shape: Some(embedding_contract.doc_shape.clone()),
            semantic_policy_version: Some(SEMANTIC_POLICY_VERSION.to_string()),
            dense_reason: Some(doc.dense_reason.as_str().to_string()),
            embedding,
            updated_at_epoch_ms,
        })
        .collect::<Vec<_>>();

    let upsert_started = Instant::now();
    storage
        .upsert_llm_symbol_docs_batch(&docs)
        .map_err(|e| ApiError::internal(format!("Failed to upsert LLM symbol docs: {e}")))?;
    stats.db_upsert_ms = stats
        .db_upsert_ms
        .saturating_add(clamp_u128_to_u32(upsert_started.elapsed().as_millis()));
    stats.docs_embedded = stats
        .docs_embedded
        .saturating_add(clamp_usize_to_u32(docs.len()));

    Ok(())
}

fn flush_streaming_llm_symbol_doc_window(
    storage: &mut Storage,
    engine: &mut SearchEngine,
    pending_docs: &mut Vec<PendingLlmSymbolDoc>,
    embed_batch_size: usize,
    embedding_contract: &EmbeddingProfileContractDto,
    updated_at_epoch_ms: i64,
    stats: &mut SemanticProjectionStats,
) -> Result<(), ApiError> {
    if pending_docs.len() < embed_batch_size {
        return Ok(());
    }

    sort_pending_llm_symbol_docs_for_embedding_batches(pending_docs);
    flush_pending_llm_symbol_docs(
        storage,
        engine,
        &pending_docs[..embed_batch_size],
        embedding_contract,
        updated_at_epoch_ms,
        stats,
    )?;
    pending_docs.drain(..embed_batch_size);
    Ok(())
}

fn sync_llm_symbol_projection(
    storage: &mut Storage,
    nodes: &[codestory_contracts::graph::Node],
    node_names: &HashMap<codestory_contracts::graph::NodeId, String>,
    engine: &mut SearchEngine,
    llm_refresh_file_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
    hydrate_semantic_docs: bool,
) -> Result<SemanticProjectionStats, ApiError> {
    let mut stats = SemanticProjectionStats {
        reported: true,
        ..Default::default()
    };

    if !hybrid_retrieval_enabled() {
        if hydrate_semantic_docs {
            engine.index_llm_symbol_docs(Vec::new());
        }
        return Ok(stats);
    }

    let embedding_contract = match engine.set_embedding_runtime_from_env() {
        Ok(()) => Some(current_embedding_contract_from_env().ok_or_else(|| {
            ApiError::internal(
                "Failed to resolve current embedding profile contract after configuring runtime",
            )
        })?),
        Err(error) => {
            tracing::warn!(
                "embedding runtime unavailable ({error}); graph-native symbol docs will still be refreshed, but dense anchor retrieval will be unavailable until managed ONNX assets are installed with `codestory-cli setup embeddings` or embedding env points at a reachable runtime. Agent-facing retrieval must be repaired to full sidecar readiness before packet/search evidence is trusted."
            );
            None
        }
    };
    let updated_at_epoch_ms = current_epoch_ms();

    let existing_docs = storage
        .get_llm_symbol_doc_reuse_metadata()
        .map_err(|e| ApiError::internal(format!("Failed to load semantic doc metadata: {e}")))?
        .into_iter()
        .map(|doc| (doc.node_id, doc))
        .collect::<HashMap<_, _>>();

    let expand_semantic_scope_for_contract_repair =
        if let Some(embedding_contract) = embedding_contract.as_ref() {
            llm_refresh_file_scope.is_some()
                && existing_docs.values().any(|existing_doc| {
                    !llm_symbol_doc_contract_matches(existing_doc, embedding_contract)
                })
        } else {
            false
        };
    if expand_semantic_scope_for_contract_repair {
        tracing::warn!(
            "Stored semantic-doc contract differs from current embedding contract; expanding incremental semantic sync to rebuild all semantic docs"
        );
    }
    let effective_llm_refresh_file_scope = if expand_semantic_scope_for_contract_repair {
        None
    } else {
        llm_refresh_file_scope
    };

    if let Some(scope) = effective_llm_refresh_file_scope
        && scope.is_empty()
    {
        if hydrate_semantic_docs {
            let reload_started = Instant::now();
            reload_llm_docs_from_storage(storage, engine, LLM_DOC_RELOAD_BATCH_SIZE)?;
            stats.reload_ms = clamp_u128_to_u32(reload_started.elapsed().as_millis());
        }
        return Ok(stats);
    }

    let embed_batch_size = llm_doc_embed_batch_size();
    let stream_pending_docs = stream_pending_llm_symbol_docs_from_env();
    let stream_sort_window_batches = semantic_stream_sort_window_batches_from_env();
    let stream_sort_window_size = embed_batch_size.saturating_mul(stream_sort_window_batches);
    tracing::debug!(embed_batch_size, "Using semantic doc embedding batch size");
    let mut pending_docs = Vec::<PendingLlmSymbolDoc>::new();
    let mut seen_symbol_node_ids = Vec::<codestory_contracts::graph::NodeId>::new();
    let mut seen_dense_node_ids = Vec::<codestory_contracts::graph::NodeId>::new();
    let mut doc_build_ns = 0_u128;
    let semantic_nodes = nodes
        .iter()
        .filter(|node| llm_indexable_kind(node.kind))
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
    let semantic_node_ids = semantic_nodes
        .iter()
        .map(|node| node.id)
        .collect::<Vec<_>>();
    let component_access = storage
        .get_component_access_map_for_nodes(&semantic_node_ids)
        .map_err(|e| ApiError::internal(format!("Failed to load symbol access metadata: {e}")))?;
    let graph_context = SemanticDocGraphContext::build(storage, &semantic_nodes, nodes)?;
    let file_cache_started = Instant::now();
    let file_text_cache = build_semantic_file_text_cache(&graph_context, &semantic_nodes);
    doc_build_ns = doc_build_ns.saturating_add(file_cache_started.elapsed().as_nanos());

    for semantic_window in semantic_nodes.chunks(stream_sort_window_size.max(1)) {
        let doc_build_started = Instant::now();
        let built_docs = semantic_window
            .par_iter()
            .map(|node| {
                let display_name = node_names
                    .get(&node.id)
                    .cloned()
                    .unwrap_or_else(|| node_display_name(node));
                let file_path = graph_context
                    .file_path_for_node(node)
                    .map(ToString::to_string);
                let doc_text = build_llm_symbol_doc_text(
                    &graph_context,
                    node,
                    &display_name,
                    file_path.as_deref(),
                    &file_text_cache,
                );
                let doc_hash = llm_symbol_doc_hash(&doc_text);
                let dense_reason = dense_anchor_reason_for_node(
                    &graph_context,
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
                let pending_with_reuse =
                    embedding_contract.as_ref().and_then(|embedding_contract| {
                        dense_reason.map(|dense_reason| {
                            let reusable =
                                existing_docs.get(&node.id).is_some_and(|existing_doc| {
                                    llm_symbol_doc_can_reuse(
                                        existing_doc,
                                        &doc_hash,
                                        embedding_contract,
                                    ) && existing_doc.dense_reason.as_deref()
                                        == Some(dense_reason.as_str())
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
                                    doc_text,
                                    doc_hash,
                                    dense_reason,
                                },
                                reusable,
                            )
                        })
                    });
                let (pending, reusable) = pending_with_reuse
                    .map(|(pending, reusable)| (Some(pending), reusable))
                    .unwrap_or((None, false));

                BuiltLlmSymbolDoc {
                    symbol_doc,
                    pending,
                    reusable,
                }
            })
            .collect::<Vec<_>>();
        doc_build_ns = doc_build_ns.saturating_add(doc_build_started.elapsed().as_nanos());

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
            observe_dense_anchor_reason(&mut stats, pending_doc.dense_reason);
            if built_doc.reusable {
                stats.docs_reused = stats.docs_reused.saturating_add(1);
                continue;
            }

            stats.docs_pending = stats.docs_pending.saturating_add(1);
            pending_docs.push(pending_doc);
        }

        while stream_pending_docs && pending_docs.len() >= embed_batch_size {
            let Some(embedding_contract) = embedding_contract.as_ref() else {
                break;
            };
            flush_streaming_llm_symbol_doc_window(
                storage,
                engine,
                &mut pending_docs,
                embed_batch_size,
                embedding_contract,
                updated_at_epoch_ms,
                &mut stats,
            )?;
        }
    }

    if effective_llm_refresh_file_scope.is_none() {
        let report_build_started = Instant::now();
        let built_reports = build_component_report_docs(
            &graph_context,
            &semantic_nodes,
            &existing_docs,
            embedding_contract.as_ref(),
            updated_at_epoch_ms,
        );
        doc_build_ns = doc_build_ns.saturating_add(report_build_started.elapsed().as_nanos());
        if !built_reports.is_empty() {
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
                .map_err(|e| {
                    ApiError::internal(format!("Failed to upsert component report nodes: {e}"))
                })?;
            let symbol_upsert_started = Instant::now();
            storage
                .upsert_symbol_search_docs_batch(&report_symbol_docs)
                .map_err(|e| {
                    ApiError::internal(format!("Failed to upsert component report docs: {e}"))
                })?;
            stats.db_upsert_ms = stats.db_upsert_ms.saturating_add(clamp_u128_to_u32(
                symbol_upsert_started.elapsed().as_millis(),
            ));
            stats.symbol_search_docs_written = stats
                .symbol_search_docs_written
                .saturating_add(clamp_usize_to_u32(report_symbol_docs.len()));

            for built_doc in built_reports {
                seen_symbol_node_ids.push(built_doc.symbol_doc.node_id);
                let Some(pending_doc) = built_doc.pending else {
                    stats.dense_docs_skipped = stats.dense_docs_skipped.saturating_add(1);
                    continue;
                };
                seen_dense_node_ids.push(pending_doc.node_id);
                observe_dense_anchor_reason(&mut stats, pending_doc.dense_reason);
                if built_doc.reusable {
                    stats.docs_reused = stats.docs_reused.saturating_add(1);
                    continue;
                }
                stats.docs_pending = stats.docs_pending.saturating_add(1);
                pending_docs.push(pending_doc);
            }

            while stream_pending_docs && pending_docs.len() >= embed_batch_size {
                let Some(embedding_contract) = embedding_contract.as_ref() else {
                    break;
                };
                flush_streaming_llm_symbol_doc_window(
                    storage,
                    engine,
                    &mut pending_docs,
                    embed_batch_size,
                    embedding_contract,
                    updated_at_epoch_ms,
                    &mut stats,
                )?;
            }
        }
    }
    stats.doc_build_ms = clamp_u128_to_u32(doc_build_ns / 1_000_000);

    if !stream_pending_docs {
        sort_pending_llm_symbol_docs_for_embedding_batches(&mut pending_docs);
    }
    if let Some(embedding_contract) = embedding_contract.as_ref() {
        for batch in pending_docs.chunks(embed_batch_size) {
            flush_pending_llm_symbol_docs(
                storage,
                engine,
                batch,
                embedding_contract,
                updated_at_epoch_ms,
                &mut stats,
            )?;
        }
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
    let stale_dense_docs = if embedding_contract.is_some() {
        if let Some(scope) = effective_llm_refresh_file_scope {
            let file_node_ids = scope.iter().copied().collect::<Vec<_>>();
            storage
                .delete_llm_symbol_docs_for_files_except_node_ids(
                    &file_node_ids,
                    &seen_dense_node_ids,
                )
                .map_err(|e| ApiError::internal(format!("Failed to prune stale LLM docs: {e}")))?
        } else {
            storage
                .prune_llm_symbol_docs_to_node_ids(&seen_dense_node_ids)
                .map_err(|e| ApiError::internal(format!("Failed to prune stale LLM docs: {e}")))?
        }
    } else {
        0
    };
    stats.prune_ms = clamp_u128_to_u32(prune_started.elapsed().as_millis());
    stats.docs_stale = clamp_usize_to_u32(stale_dense_docs.saturating_add(stale_symbol_docs));

    if hydrate_semantic_docs {
        let reload_started = Instant::now();
        reload_llm_docs_from_storage(storage, engine, LLM_DOC_RELOAD_BATCH_SIZE)?;
        stats.reload_ms = clamp_u128_to_u32(reload_started.elapsed().as_millis());
    }

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
    } else if normalized.contains("/data/indexer/")
        || architecture_has_all_terms(&terms, &["indexer", "command"])
    {
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
    value: IndexFreshnessDto,
    cached_at: Instant,
}

struct AppState {
    project_root: Option<PathBuf>,
    storage_path: Option<PathBuf>,
    node_names: HashMap<codestory_contracts::graph::NodeId, String>,
    search_engine: Option<SearchEngine>,
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

fn publish_search_engine(state: &mut AppState, engine: SearchEngine) {
    state.index_freshness_cache = None;
    state.search_engine = Some(engine);
}

fn clear_search_engine(state: &mut AppState) {
    state.search_engine = None;
}

/// GUI-agnostic orchestrator for CodeStory.
///
/// This is intentionally "headless": any app shell (CLI, desktop, IDE integration)
/// should call methods on this controller and subscribe to `AppEventPayload`.
#[derive(Clone)]
pub struct AppController {
    state: Arc<Mutex<AppState>>,
    sidecar_query_cache: Arc<Mutex<codestory_retrieval::RetrievalCache>>,
    grounding_detail_refresh: Arc<Mutex<()>>,
    events_tx: Sender<AppEventPayload>,
    events_rx: Receiver<AppEventPayload>,
}

impl Default for AppController {
    fn default() -> Self {
        Self::new()
    }
}

impl AppController {
    pub fn new() -> Self {
        let (events_tx, events_rx) = unbounded();
        Self {
            state: Arc::new(Mutex::new(AppState {
                project_root: None,
                storage_path: None,
                node_names: HashMap::new(),
                search_engine: None,
                is_indexing: false,
                index_freshness_cache: None,
                #[cfg(test)]
                last_hybrid_instrumentation: None,
            })),
            sidecar_query_cache: Arc::new(Mutex::new(codestory_retrieval::RetrievalCache::new())),
            grounding_detail_refresh: Arc::new(Mutex::new(())),
            events_tx,
            events_rx,
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
        ReadOnlyBrowserService::new(self.clone())
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

    fn open_storage(&self) -> Result<Storage, ApiError> {
        let storage_path = self.require_storage_path()?;
        Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))
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
        {
            let s = self.state.lock();
            if s.search_engine.is_some() {
                return Ok(());
            }
        }

        let storage_path = self.require_storage_path()?;
        let mut storage = Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;
        let (node_names, engine) = load_persisted_search_state(&mut storage, &storage_path)?;

        let mut s = self.state.lock();
        if s.search_engine.is_none() {
            s.node_names = node_names;
            publish_search_engine(&mut s, engine);
        }

        Ok(())
    }

    pub fn retrieval_state(&self) -> Result<RetrievalStateDto, ApiError> {
        let storage = self.open_storage()?;
        retrieval_state_from_storage(&storage)
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
    ) -> Option<SearchHit> {
        let node = match storage.get_node(id) {
            Ok(Some(node)) if node.kind != codestory_contracts::graph::NodeKind::UNKNOWN => node,
            _ => return None,
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

        Some(SearchHit {
            node_id: NodeId::from(id),
            display_name,
            kind: NodeKind::from(node.kind),
            file_path,
            line,
            score: route_endpoint_adjusted_search_score(score, node.canonical_id.as_deref()),
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph),
            evidence_producer: Some("route_endpoint".to_string()),
            resolution_status: Some(
                codestory_contracts::api::PacketEvidenceResolutionDto::Resolved,
            ),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
            score_breakdown: None,
        })
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
        let storage = self.open_storage()?;
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
            .filter_map(|(id, score)| Self::build_search_hit(&storage, &node_names, id, score))
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
        let workspace = Workspace::open(root.to_path_buf())
            .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;
        let members = workspace_member_storage_summaries(root, &workspace, storage)?;
        let freshness = index_freshness_from_storage(root, &workspace, storage);

        Ok(ProjectSummary {
            root: root.to_string_lossy().to_string(),
            stats: dto_stats,
            members,
            retrieval: Some(retrieval_state_from_storage(storage)?),
            freshness: Some(freshness),
        })
    }

    fn open_project_summary_with_storage_inner(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
    ) -> Result<ProjectSummary, ApiError> {
        let storage = Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;
        let summary = self.project_summary_from_storage(&root, &storage)?;

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
        let mut storage = Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;
        let (node_names, engine) = load_persisted_search_state(&mut storage, &storage_path)?;
        let mut summary = self.project_summary_from_storage(&root, &storage)?;
        summary.retrieval = Some(retrieval_state_from_storage(&storage)?);

        {
            let mut s = self.state.lock();
            s.project_root = Some(root);
            s.storage_path = Some(storage_path);
            s.node_names = node_names;
            publish_search_engine(&mut s, engine);
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

    pub fn start_indexing(&self, req: StartIndexingRequest) -> Result<(), ApiError> {
        let (root, storage_path) = {
            let mut s = self.state.lock();
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
            s.is_indexing = true;
            (root, storage_path)
        };

        let events_tx = self.events_tx.clone();
        let controller = self.clone();

        // Use a dedicated thread so callers can keep their runtime responsive.
        std::thread::spawn(move || {
            let indexing_started = std::time::Instant::now();
            let result = match req.mode {
                IndexMode::Full => index_full(&root, &storage_path, &events_tx),
                IndexMode::Incremental => index_incremental(&root, &storage_path, &events_tx),
            };

            match result {
                Ok(summary) => {
                    controller.clear_search_state();
                    controller.state.lock().is_indexing = false;
                    let _ = events_tx.send(AppEventPayload::IndexingComplete {
                        duration_ms: clamp_u128_to_u32(indexing_started.elapsed().as_millis()),
                        phase_timings: summary.phase_timings,
                    });
                }
                Err(err) => {
                    let _ = events_tx.send(AppEventPayload::IndexingFailed { error: err.message });
                    controller.clear_search_state();
                    controller.state.lock().is_indexing = false;
                }
            }
        });

        Ok(())
    }

    fn run_indexing_blocking_inner(
        &self,
        mode: IndexMode,
        refresh_runtime_caches: bool,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        let (root, storage_path) = {
            let mut s = self.state.lock();
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
            s.is_indexing = true;
            (root, storage_path)
        };

        let result = match mode {
            IndexMode::Full => index_full(&root, &storage_path, &self.events_tx),
            IndexMode::Incremental => index_incremental(&root, &storage_path, &self.events_tx),
        };

        match result {
            Ok(summary) => {
                self.finish_successful_indexing(summary, &storage_path, refresh_runtime_caches)
            }
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
    ) -> Result<IndexingPhaseTimings, ApiError> {
        let cache_refresh_started = Instant::now();
        let cache_stats = if refresh_runtime_caches {
            let mut storage = Storage::open(storage_path)
                .map_err(|e| ApiError::internal(format!("Failed to reopen storage: {e}")))?;
            refresh_caches(
                self,
                &mut storage,
                storage_path,
                summary.llm_refresh_scope.as_ref(),
            )?
        } else {
            self.finalize_indexing_without_runtime_refresh_with(
                storage_path,
                summary.llm_refresh_scope.as_ref(),
                |storage, llm_refresh_scope| {
                    rebuild_search_state_from_storage(
                        storage,
                        storage_path,
                        llm_refresh_scope,
                        false,
                    )
                    .map(|result| CacheRefreshStats {
                        search_stats: result.search_stats,
                        semantic_stats: result.semantic_stats,
                        runtime_cache_publish_ms: None,
                    })
                },
            )?
        };
        summary.phase_timings.cache_refresh_ms = Some(clamp_u128_to_u32(
            cache_refresh_started.elapsed().as_millis(),
        ));
        apply_cache_refresh_stats(&mut summary.phase_timings, cache_stats);
        Ok(summary.phase_timings)
    }

    fn recover_failed_indexing(&self, storage_path: &Path, refresh_runtime_caches: bool) {
        if refresh_runtime_caches && let Ok(mut storage) = Storage::open(storage_path) {
            let _ = refresh_caches(self, &mut storage, storage_path, None);
            return;
        }
        self.clear_search_state();
        self.state.lock().is_indexing = false;
    }

    pub fn run_indexing_blocking(&self, mode: IndexMode) -> Result<IndexingPhaseTimings, ApiError> {
        self.run_indexing_blocking_inner(mode, true)
    }

    pub fn run_indexing_blocking_without_runtime_refresh(
        &self,
        mode: IndexMode,
    ) -> Result<IndexingPhaseTimings, ApiError> {
        self.run_indexing_blocking_inner(mode, false)
    }

    pub fn dry_run_index(&self, mode: IndexMode) -> Result<IndexDryRunDto, ApiError> {
        let root = self.require_project_root()?;
        let storage_path = self.require_storage_path()?;
        let workspace = Workspace::open(root.clone())
            .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;
        let refresh_inputs = if storage_path.exists() {
            let store = Store::open(&storage_path)
                .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;
            workspace_refresh_inputs(&store)?
        } else {
            RefreshInputs::default()
        };
        let execution_plan = match mode {
            IndexMode::Full => workspace.full_refresh_execution_plan().map_err(|e| {
                ApiError::internal(format!("Failed to generate full refresh plan: {e}"))
            })?,
            IndexMode::Incremental => {
                workspace
                    .build_execution_plan(&refresh_inputs)
                    .map_err(|e| {
                        ApiError::internal(format!(
                            "Failed to generate incremental refresh plan: {e}"
                        ))
                    })?
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
        let endpoint = std::env::var("CODESTORY_SUMMARY_ENDPOINT")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ApiError::invalid_argument(
                    "--summarize requires CODESTORY_SUMMARY_ENDPOINT to be configured.",
                )
            })?;
        let model = std::env::var("CODESTORY_SUMMARY_MODEL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "codestory-symbol-summary".to_string());
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
            let summary = summarize_symbol_doc(&endpoint, &model, &doc)?;
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

    pub fn search(&self, req: SearchRequest) -> Result<Vec<SearchHit>, ApiError> {
        Ok(self.search_results(req)?.hits)
    }

    pub fn search_results(&self, req: SearchRequest) -> Result<SearchResultsDto, ApiError> {
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
                    "sidecar retrieval primary is mandatory; legacy search is disabled".to_string()
                });
            return Err(
                agent::retrieval_primary::sidecar_retrieval_unavailable_error(self, reason),
            );
        }

        let query = intent_query.effective_query.clone();
        let query_result = agent::retrieval_primary::run_sidecar_query(self, &query, None)
            .map_err(|error| {
                agent::retrieval_primary::sidecar_retrieval_unavailable_error(
                    self,
                    format!("sidecar search failed: {error}"),
                )
            })?;
        let mut indexed_symbol_hits =
            agent::retrieval_primary::resolve_sidecar_candidates_to_search_hits(
                self,
                &query_result.hits,
                limit_per_source,
            )
            .map_err(|error| {
                agent::retrieval_primary::sidecar_retrieval_unavailable_error(
                    self,
                    format!(
                        "sidecar search rejected query: candidate resolution failed: {}",
                        error.message
                    ),
                )
            })?;
        if let Some(reason) = agent::retrieval_primary::sidecar_result_rejection_reason(
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
        let candidate_count = query_result.hits.len();
        let resolved_hit_count = indexed_symbol_hits.len();
        let initial_sidecar_hits = indexed_symbol_hits.clone();

        apply_search_intent_filters(&mut indexed_symbol_hits, &intent_query.filters);
        let project_root = self.require_project_root().ok();
        indexed_symbol_hits.sort_by(|left, right| {
            compare_search_hits_with_project_root(project_root.as_deref(), &query, left, right)
        });
        dedupe_inexact_search_hits_by_display_key(&query, &mut indexed_symbol_hits);
        indexed_symbol_hits.truncate(limit_per_source);
        annotate_search_hit_match_quality(&query, &mut indexed_symbol_hits);

        let storage = self.open_storage()?;
        let retrieval = retrieval_state_from_storage(&storage)?;
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
                candidate_count,
                resolved_hit_count,
                &initial_sidecar_hits,
                &hits,
            ),
        );

        Ok(SearchResultsDto {
            query: original_query,
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
        let storage = self.open_storage()?;
        let mut files = storage
            .get_files()
            .map_err(|e| ApiError::internal(format!("Failed to load indexed files: {e}")))?;
        files.sort_by(|left, right| left.path.cmp(&right.path));

        let errors = storage
            .get_errors(None)
            .map_err(|e| ApiError::internal(format!("Failed to load index errors: {e}")))?;
        let mut errors_by_file = HashMap::<i64, u32>::new();
        for error in errors {
            if let Some(file_id) = error.file_id {
                *errors_by_file.entry(file_id.0).or_default() += 1;
            }
        }

        let mut language_counts = BTreeMap::<String, u32>::new();
        let mut indexed_file_count = 0_u32;
        let mut incomplete_file_count = 0_u32;
        let mut error_file_count = 0_u32;
        for file in &files {
            *language_counts.entry(file.language.clone()).or_default() += 1;
            indexed_file_count += u32::from(file.indexed);
            incomplete_file_count += u32::from(!file.complete);
            error_file_count += u32::from(errors_by_file.contains_key(&file.id));
        }

        let path_filter = req.path_contains.as_deref().map(normalize_path_key);
        let language_filter = req.language.as_deref().map(str::to_ascii_lowercase);
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
                truncated,
                language_counts,
                framework_route_coverage: framework_route_coverage_matrix(),
                coverage_notes,
            },
            files: visible,
        })
    }

    pub fn affected_analysis(
        &self,
        req: AffectedAnalysisRequest,
    ) -> Result<AffectedAnalysisDto, ApiError> {
        self.ensure_consistent_read_state("Affected analysis")?;
        let root = self.require_project_root()?;
        let depth = req.depth.unwrap_or(2).clamp(1, 8);
        let filter = req.filter.as_deref().map(normalize_path_key);
        let storage = self.open_storage()?;
        let files = storage
            .get_files()
            .map_err(|e| ApiError::internal(format!("Failed to load indexed files: {e}")))?;
        let nodes = storage
            .get_nodes()
            .map_err(|e| ApiError::internal(format!("Failed to load graph nodes: {e}")))?;
        let edges = storage
            .get_edges()
            .map_err(|e| ApiError::internal(format!("Failed to load graph edges: {e}")))?;
        let errors = storage
            .get_errors(None)
            .map_err(|e| ApiError::internal(format!("Failed to load index errors: {e}")))?;
        let mut errors_by_file = HashMap::<i64, u32>::new();
        for error in errors {
            if let Some(file_id) = error.file_id {
                *errors_by_file.entry(file_id.0).or_default() += 1;
            }
        }

        let change_records = normalized_affected_change_records(&req);
        let mut matched_file_ids = HashSet::<GraphNodeId>::new();
        let mut matched_changed_keys = HashSet::<String>::new();
        let mut matched_record_by_file_id = HashMap::<GraphNodeId, AffectedChangeRecordDto>::new();
        for file in &files {
            let relative_key = normalize_path_key(&runtime_relative_path(&root, &file.path));
            let absolute_key = normalize_path_key(&file.path.to_string_lossy());
            let mut matched_records = Vec::new();
            let matched_keys = change_records
                .iter()
                .flat_map(|record| {
                    affected_change_record_keys(record)
                        .into_iter()
                        .map(move |key| (record, key))
                })
                .filter_map(|(record, changed)| {
                    let changed = changed.as_str();
                    let matched = relative_key == changed
                        || relative_key.ends_with(changed)
                        || changed.ends_with(&relative_key)
                        || absolute_key == changed
                        || absolute_key.ends_with(changed);
                    matched.then_some((record, changed.to_string()))
                })
                .collect::<Vec<_>>();
            if !matched_keys.is_empty() {
                let file_id = codestory_contracts::graph::NodeId(file.id);
                matched_file_ids.insert(file_id);
                for (record, key) in matched_keys {
                    matched_changed_keys.insert(key);
                    matched_records.push(record.clone());
                }
                if let Some(record) = matched_records.first() {
                    matched_record_by_file_id.insert(file_id, record.clone());
                }
            }
        }
        let mut matched_files = files
            .iter()
            .filter(|file| matched_file_ids.contains(&codestory_contracts::graph::NodeId(file.id)))
            .map(|file| {
                let file_id = codestory_contracts::graph::NodeId(file.id);
                let record = matched_record_by_file_id.get(&file_id);
                AffectedMatchedFileDto {
                    path: runtime_relative_path(&root, &file.path),
                    role: indexed_file_role(&file.path),
                    indexed: file.indexed,
                    complete: file.complete,
                    change_kind: record.map(|record| record.kind.clone()),
                    change_status: record.map(|record| record.status.clone()),
                    previous_path: record.and_then(|record| record.previous_path.clone()),
                    error_count: errors_by_file.get(&file.id).copied().unwrap_or_default(),
                }
            })
            .collect::<Vec<_>>();
        matched_files.sort_by(|left, right| left.path.cmp(&right.path));
        let unmatched_paths = change_records
            .iter()
            .filter(|record| {
                affected_change_record_keys(record)
                    .iter()
                    .all(|key| !matched_changed_keys.contains(key))
            })
            .map(|record| AffectedUnmatchedPathDto {
                path: record.path.clone(),
                reason: affected_unmatched_reason(record),
                change_kind: Some(record.kind.clone()),
                change_status: Some(record.status.clone()),
                previous_path: record.previous_path.clone(),
            })
            .collect::<Vec<_>>();

        let mut labels = self.cached_labels(nodes.iter().map(|node| node.id));
        for node in &nodes {
            labels.entry(node.id).or_insert_with(|| {
                node.qualified_name
                    .clone()
                    .unwrap_or_else(|| node.serialized_name.clone())
            });
        }
        let file_path_by_id = files
            .iter()
            .map(|file| {
                (
                    codestory_contracts::graph::NodeId(file.id),
                    runtime_relative_path(&root, &file.path),
                )
            })
            .collect::<HashMap<_, _>>();
        let nodes_by_id = nodes
            .iter()
            .map(|node| (node.id, node.clone()))
            .collect::<HashMap<_, _>>();
        let mut node_ids_by_file = HashMap::<GraphNodeId, Vec<GraphNodeId>>::new();
        for node in &nodes {
            if let Some(file_id) = node.file_node_id {
                node_ids_by_file.entry(file_id).or_default().push(node.id);
            }
        }

        let mut seeds = HashSet::<GraphNodeId>::new();
        for file_id in &matched_file_ids {
            seeds.insert(*file_id);
            if let Some(file_nodes) = node_ids_by_file.get(file_id) {
                seeds.extend(file_nodes.iter().copied());
            }
        }

        let mut reverse_dependents = HashMap::<GraphNodeId, Vec<(GraphNodeId, usize)>>::new();
        for (edge_index, edge) in edges.iter().enumerate() {
            let source = edge.effective_source();
            let target = edge.effective_target();
            reverse_dependents
                .entry(target)
                .or_default()
                .push((source, edge_index));
        }

        let mut distances = HashMap::<GraphNodeId, u32>::new();
        let mut evidence = HashMap::<GraphNodeId, AffectedGraphEvidence>::new();
        let mut queue = VecDeque::<(GraphNodeId, u32)>::new();
        for seed in seeds {
            distances.insert(seed, 0);
            let seed_reason = nodes_by_id
                .get(&seed)
                .map(|node| {
                    if node.kind == codestory_contracts::graph::NodeKind::FILE {
                        "changed file matched input path"
                    } else {
                        "symbol declared in changed file"
                    }
                })
                .unwrap_or("changed path seed");
            evidence.insert(
                seed,
                AffectedGraphEvidence {
                    distance: 0,
                    reason: seed_reason.to_string(),
                    confidence: "direct".to_string(),
                },
            );
            queue.push_back((seed, 0));
        }
        while let Some((node_id, distance)) = queue.pop_front() {
            if distance >= depth {
                continue;
            }
            for (dependent, edge_id) in reverse_dependents.get(&node_id).into_iter().flatten() {
                let next_distance = distance + 1;
                if distances
                    .get(dependent)
                    .is_none_or(|current| next_distance < *current)
                {
                    distances.insert(*dependent, next_distance);
                    let edge = edges.get(*edge_id);
                    let target_label = labels
                        .get(&node_id)
                        .cloned()
                        .unwrap_or_else(|| node_id.0.to_string());
                    evidence.insert(
                        *dependent,
                        affected_dependent_evidence(next_distance, edge, target_label),
                    );
                    queue.push_back((*dependent, next_distance));
                }
            }
        }

        let mut impacted_symbols = distances
            .iter()
            .filter_map(|(node_id, distance)| {
                let node = nodes_by_id.get(node_id)?;
                if node.kind == codestory_contracts::graph::NodeKind::FILE {
                    return None;
                }
                let file_path = node
                    .file_node_id
                    .and_then(|file_id| file_path_by_id.get(&file_id).cloned());
                if filter.as_deref().is_some_and(|needle| {
                    !labels
                        .get(node_id)
                        .is_some_and(|label| normalize_path_key(label).contains(needle))
                        && !file_path
                            .as_deref()
                            .is_some_and(|path| normalize_path_key(path).contains(needle))
                }) {
                    return None;
                }
                let graph_evidence =
                    evidence
                        .get(node_id)
                        .cloned()
                        .unwrap_or_else(|| AffectedGraphEvidence {
                            distance: *distance,
                            reason: "reached by dependent graph walk".to_string(),
                            confidence: "graph".to_string(),
                        });
                Some(AffectedSymbolDto {
                    node_id: NodeId::from(*node_id),
                    display_name: labels
                        .get(node_id)
                        .cloned()
                        .unwrap_or_else(|| node.serialized_name.clone()),
                    kind: NodeKind::from(node.kind),
                    file_path,
                    line: node.start_line,
                    distance: *distance,
                    graph_depth: graph_evidence.distance,
                    reason: graph_evidence.reason,
                    confidence: graph_evidence.confidence,
                })
            })
            .collect::<Vec<_>>();
        impacted_symbols.sort_by(|left, right| {
            left.distance
                .cmp(&right.distance)
                .then(left.file_path.cmp(&right.file_path))
                .then(left.display_name.cmp(&right.display_name))
        });
        impacted_symbols.truncate(200);

        let mut impacted_routes = distances
            .iter()
            .filter_map(|(node_id, distance)| {
                let node = nodes_by_id.get(node_id)?;
                let file_path = node
                    .file_node_id
                    .and_then(|file_id| file_path_by_id.get(&file_id).cloned());
                if filter.as_deref().is_some_and(|needle| {
                    !labels
                        .get(node_id)
                        .is_some_and(|label| normalize_path_key(label).contains(needle))
                        && !file_path
                            .as_deref()
                            .is_some_and(|path| normalize_path_key(path).contains(needle))
                }) {
                    return None;
                }
                let display_name = labels
                    .get(node_id)
                    .cloned()
                    .unwrap_or_else(|| node.serialized_name.clone());
                let route = self.route_endpoint_metadata(
                    &storage,
                    node,
                    file_path.as_deref(),
                    &display_name,
                )?;
                let graph_evidence =
                    evidence
                        .get(node_id)
                        .cloned()
                        .unwrap_or_else(|| AffectedGraphEvidence {
                            distance: *distance,
                            reason: "route endpoint reached by dependent graph walk".to_string(),
                            confidence: route
                                .confidence
                                .clone()
                                .unwrap_or_else(|| "graph".to_string()),
                        });
                let confidence = route
                    .confidence
                    .clone()
                    .unwrap_or_else(|| graph_evidence.confidence.clone());
                Some(AffectedRouteDto {
                    node_id: NodeId::from(*node_id),
                    display_name,
                    file_path,
                    line: node.start_line,
                    distance: *distance,
                    graph_depth: graph_evidence.distance,
                    reason: graph_evidence.reason,
                    confidence,
                    route,
                })
            })
            .collect::<Vec<_>>();
        impacted_routes.sort_by(|left, right| {
            left.distance
                .cmp(&right.distance)
                .then(left.file_path.cmp(&right.file_path))
                .then(left.display_name.cmp(&right.display_name))
        });
        impacted_routes.truncate(100);

        let mut impacted_by_test_file = BTreeMap::<String, (u32, u32, String)>::new();
        for symbol in &impacted_symbols {
            if let Some(path) = symbol.file_path.as_deref()
                && path_role_from_key(&normalize_path_key(path)) == IndexedFileRoleDto::Test
            {
                let entry = impacted_by_test_file.entry(path.to_string()).or_insert((
                    0,
                    symbol.graph_depth,
                    symbol.confidence.clone(),
                ));
                entry.0 += 1;
                if symbol.graph_depth < entry.1 {
                    entry.1 = symbol.graph_depth;
                    entry.2.clone_from(&symbol.confidence);
                }
            }
        }
        let impacted_tests = impacted_by_test_file
            .into_iter()
            .map(
                |(path, (impacted_symbol_count, distance, confidence))| AffectedTestFileDto {
                    path,
                    reason: "focused test hint: test-like path reached by affected graph walk"
                        .to_string(),
                    confidence,
                    distance,
                    graph_depth: distance,
                    impacted_symbol_count,
                },
            )
            .collect::<Vec<_>>();

        let mut notes = Vec::new();
        let mut blind_spots = Vec::new();
        if matched_file_ids.is_empty() {
            let note =
                "no changed paths matched indexed files; pass repo-relative paths or reindex first"
                    .to_string();
            notes.push(note.clone());
            blind_spots.push(note);
        } else {
            notes.push(format!(
                "matched {} indexed files; dependency walk expanded files into contained symbols",
                matched_file_ids.len()
            ));
        }
        if !unmatched_paths.is_empty() {
            blind_spots.push(format!(
                "{} changed paths were unmatched and excluded from graph traversal",
                unmatched_paths.len()
            ));
        }
        if matched_files
            .iter()
            .any(|file| !file.complete || file.error_count > 0)
        {
            blind_spots.push(
                "one or more matched files are incomplete or have recorded index errors"
                    .to_string(),
            );
        }
        if impacted_routes.is_empty() {
            blind_spots.push(
                "no route/endpoint evidence found for matched files or dependents".to_string(),
            );
        }
        if impacted_tests.is_empty() {
            notes.push("no impacted test-like files found in the indexed graph".to_string());
        }
        let project = root.to_string_lossy().to_string();
        let next_commands = vec![
            format!("codestory-cli files --project \"{project}\" --format markdown"),
            format!("codestory-cli doctor --project \"{project}\" --format markdown"),
            format!("codestory-cli index --project \"{project}\" --refresh full"),
        ];

        Ok(AffectedAnalysisDto {
            project_root: project,
            changed_paths: req.changed_paths,
            change_records,
            matched_files,
            unmatched_paths,
            matched_file_count: matched_file_ids.len().min(u32::MAX as usize) as u32,
            depth,
            impacted_symbols,
            impacted_routes,
            impacted_tests,
            blind_spots,
            next_commands,
            notes,
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
            .filter_map(|(id, score)| Self::build_search_hit(storage, &node_names, id, score))
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
        let ttl = Duration::from_secs(index_freshness_cache_ttl_secs());
        {
            let state = self.state.lock();
            if let Some(cached) = state.index_freshness_cache.as_ref()
                && cached.cached_at.elapsed() < ttl
            {
                return Ok(cached.value.clone());
            }
        }

        let root = self.require_project_root()?;
        let storage = self.open_storage()?;
        let workspace = Workspace::open(root.clone())
            .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;
        let freshness = index_freshness_from_storage(&root, &workspace, &storage);
        let mut state = self.state.lock();
        state.index_freshness_cache = Some(CachedIndexFreshness {
            value: freshness.clone(),
            cached_at: Instant::now(),
        });
        Ok(freshness)
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
        let storage = self.open_storage()?;
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
                    && let Err(error) = engine.set_embedding_runtime_from_env()
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
                Self::build_search_hit(&storage, &node_names, scored.node_id, scored.total_score)
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
        let storage = self.open_storage()?;

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
        let storage = self.open_storage()?;

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

    pub fn agent_ask(&self, req: AgentAskRequest) -> Result<AgentAnswerDto, ApiError> {
        agent::agent_ask(self, req)
    }

    pub fn begin_packet_retrieval(&self) {
        let _ = self;
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn take_hybrid_instrumentation(&self) -> Option<HybridSearchInstrumentation> {
        self.state.lock().last_hybrid_instrumentation.take()
    }

    pub fn agent_packet(&self, req: AgentPacketRequestDto) -> Result<AgentPacketDto, ApiError> {
        agent::agent_packet(self, req)
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
        let storage = self.open_storage()?;
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
        let storage = self.open_storage()?;
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
        let storage = self.open_storage()?;
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
        let storage = self.open_storage()?;
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

        let storage = self.open_storage()?;

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
        } else if let Some(label) = canonical_id.strip_prefix("openapi:endpoint:") {
            route_endpoint_metadata_from_openapi_label(label, node, source_file)?
        } else {
            return None;
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
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .confidence
                .partial_cmp(&left.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    route_handler_certainty_rank(right.certainty)
                        .cmp(&route_handler_certainty_rank(left.certainty))
                })
        });
        let (edge, target) = candidates.into_iter().find_map(|edge| {
            let target_id = edge.effective_target();
            let target = storage.get_node(target_id).ok().flatten()?;
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
            Some((edge, target))
        })?;
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
        let storage = self.open_storage()?;
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
        let storage = self.open_storage()?;
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

#[derive(Debug, Clone)]
struct IndexingRunSummary {
    phase_timings: IndexingPhaseTimings,
    llm_refresh_scope: Option<HashSet<codestory_contracts::graph::NodeId>>,
}

fn index_full(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
) -> Result<IndexingRunSummary, ApiError> {
    let workspace = Workspace::open(root.to_path_buf())
        .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;
    let execution_plan = workspace
        .full_refresh_execution_plan()
        .map_err(|e| ApiError::internal(format!("Failed to collect files: {e}")))?;

    let total_files = execution_plan.files_to_index.len().min(u32::MAX as usize) as u32;
    let _ = events_tx.send(AppEventPayload::IndexingStarted {
        file_count: total_files,
    });

    let mut staged = SnapshotStore::open_staged(storage_path)
        .map_err(|e| ApiError::internal(format!("Failed to open staged storage: {e}")))?;
    let can_copy_forward = storage_path.exists();

    let bus = EventBus::new();
    let forwarder = spawn_progress_forwarder(bus.receiver(), events_tx.clone());
    let indexer = V2WorkspaceIndexer::new(root.to_path_buf());
    let result = indexer.run(staged.store_mut(), &execution_plan, &bus, None);

    drop(bus);
    let _ = forwarder.join();

    let index_stats = match result {
        Ok(stats) => stats,
        Err(err) => {
            let _ = staged.discard();
            return Err(ApiError::internal(format!("Indexing failed: {err}")));
        }
    };
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
        match staged.store_mut().copy_llm_symbol_docs_from(storage_path) {
            Ok(copied) => tracing::debug!(copied, "Copied semantic docs into staged storage"),
            Err(error) => {
                tracing::warn!("Failed to copy semantic docs into staged storage: {error}")
            }
        }
    }
    let staged_finalize_stats = match staged.snapshots().finalize_staged() {
        Ok(stats) => stats,
        Err(err) => {
            let _ = staged.discard();
            return Err(ApiError::internal(format!(
                "Failed to finalize staged snapshot lifecycle: {err}"
            )));
        }
    };
    let deferred_indexes_ms = staged_finalize_stats.deferred_indexes_ms;
    let summary_snapshot_ms = staged_finalize_stats.summary_snapshot_ms;
    let staged_path = staged.path().to_path_buf();
    let publish_started = std::time::Instant::now();
    if let Err(err) = staged.publish(storage_path) {
        return Err(ApiError::internal(format!(
            "Failed to publish staged storage: {err}. Preserved staged snapshot at {}",
            staged_path.display()
        )));
    }
    let publish_ms = clamp_u128_to_u32(publish_started.elapsed().as_millis());
    let resolution_telemetry = OptionalResolutionTelemetry::from_incremental_stats(&index_stats);
    Ok(IndexingRunSummary {
        phase_timings: IndexingPhaseTimings {
            parse_index_ms: clamp_u64_to_u32(index_stats.parse_index_ms),
            projection_flush_ms: clamp_u64_to_u32(index_stats.projection_flush_ms),
            edge_resolution_ms: clamp_u64_to_u32(index_stats.edge_resolution_ms),
            error_flush_ms: clamp_u64_to_u32(index_stats.error_flush_ms),
            cleanup_ms: clamp_u64_to_u32(index_stats.cleanup_ms),
            cache_refresh_ms: None,
            search_projection_rebuild_ms: None,
            search_symbol_index_ms: None,
            runtime_cache_publish_ms: None,
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
            detail_snapshot_ms: None,
            publish_ms: Some(publish_ms),
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
        llm_refresh_scope: None,
    })
}

fn index_incremental(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
) -> Result<IndexingRunSummary, ApiError> {
    run_incremental_indexing_common(root, storage_path, events_tx, |workspace, inputs| {
        workspace
            .build_execution_plan(inputs)
            .map_err(|e| ApiError::internal(format!("Failed to generate refresh info: {e}")))
    })
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

fn run_incremental_indexing_common<F>(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    refresh_builder: F,
) -> Result<IndexingRunSummary, ApiError>
where
    F: FnOnce(&Workspace, &RefreshInputs) -> Result<RefreshExecutionPlan, ApiError>,
{
    let mut store = Store::open(storage_path)
        .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;

    let workspace = Workspace::open(root.to_path_buf())
        .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;

    let refresh_inputs = workspace_refresh_inputs(&store)?;
    let execution_plan = refresh_builder(&workspace, &refresh_inputs)?;

    let total_files = execution_plan.files_to_index.len().min(u32::MAX as usize) as u32;
    let _ = events_tx.send(AppEventPayload::IndexingStarted {
        file_count: total_files,
    });

    let bus = EventBus::new();
    let forwarder = spawn_progress_forwarder(bus.receiver(), events_tx.clone());

    let indexer = V2WorkspaceIndexer::new(root.to_path_buf());
    let result = indexer.run(&mut store, &execution_plan, &bus, None);

    // Drop bus so forwarder unblocks.
    drop(bus);
    let _ = forwarder.join();

    let index_stats = result.map_err(|e| ApiError::internal(format!("Indexing failed: {e}")))?;
    let snapshot_refresh_stats = store.snapshots().refresh_all_with_stats().map_err(|e| {
        ApiError::internal(format!("Failed to refresh live grounding snapshots: {e}"))
    })?;
    let summary_snapshot_ms = snapshot_refresh_stats.summary_snapshot_ms;
    let detail_snapshot_ms = snapshot_refresh_stats.detail_snapshot_ms;
    let resolution_telemetry = OptionalResolutionTelemetry::from_incremental_stats(&index_stats);

    let mut llm_refresh_scope = HashSet::new();
    for path in &execution_plan.files_to_index {
        let normalized_path = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        if let Ok(Some(file_info)) = store.get_file_by_path(&normalized_path) {
            llm_refresh_scope.insert(codestory_contracts::graph::NodeId(file_info.id));
        }
    }
    for file_id in &execution_plan.files_to_remove {
        llm_refresh_scope.insert(codestory_contracts::graph::NodeId(*file_id));
    }

    Ok(IndexingRunSummary {
        phase_timings: IndexingPhaseTimings {
            parse_index_ms: clamp_u64_to_u32(index_stats.parse_index_ms),
            projection_flush_ms: clamp_u64_to_u32(index_stats.projection_flush_ms),
            edge_resolution_ms: clamp_u64_to_u32(index_stats.edge_resolution_ms),
            error_flush_ms: clamp_u64_to_u32(index_stats.error_flush_ms),
            cleanup_ms: clamp_u64_to_u32(index_stats.cleanup_ms),
            cache_refresh_ms: None,
            search_projection_rebuild_ms: None,
            search_symbol_index_ms: None,
            runtime_cache_publish_ms: None,
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
            deferred_indexes_ms: None,
            summary_snapshot_ms: Some(summary_snapshot_ms),
            detail_snapshot_ms: Some(detail_snapshot_ms),
            publish_ms: None,
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
        llm_refresh_scope: Some(llm_refresh_scope),
    })
}

fn workspace_refresh_inputs(store: &Store) -> Result<RefreshInputs, ApiError> {
    let files = store
        .files()
        .get_files()
        .map_err(|e| ApiError::internal(format!("Failed to read workspace inventory: {e}")))?;
    Ok(refresh_inputs_from_files(files))
}

fn rebuild_search_state_from_storage(
    storage: &mut Storage,
    storage_path: &Path,
    llm_refresh_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
    hydrate_semantic_docs: bool,
) -> Result<SearchStateBuildResult, ApiError> {
    match storage.get_nodes() {
        Ok(nodes) => build_search_state(
            storage,
            Some(search_index_storage_path(storage_path).as_path()),
            nodes,
            llm_refresh_scope,
            SemanticProjectionMode::PersistBackedDocs,
            hydrate_semantic_docs,
        )
        .map_err(|e| ApiError::internal(format!("Failed to rebuild search state: {}", e.message))),
        Err(e) => Err(ApiError::internal(format!(
            "Failed to load nodes for search rebuild: {e}"
        ))),
    }
}

fn refresh_caches(
    controller: &AppController,
    storage: &mut Storage,
    storage_path: &Path,
    llm_refresh_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
) -> Result<CacheRefreshStats, ApiError> {
    let refreshed =
        rebuild_search_state_from_storage(storage, storage_path, llm_refresh_scope, true);

    let mut s = controller.state.lock();
    match refreshed {
        Ok(result) => {
            let publish_started = Instant::now();
            let semantic_stats = result.semantic_stats;
            let search_stats = result.search_stats;
            s.node_names = result.node_names;
            publish_search_engine(&mut s, result.engine);
            controller.sidecar_query_cache.lock().clear();
            s.is_indexing = false;
            Ok(CacheRefreshStats {
                search_stats,
                semantic_stats,
                runtime_cache_publish_ms: Some(clamp_u128_to_u32(
                    publish_started.elapsed().as_millis(),
                )),
            })
        }
        Err(error) => {
            tracing::warn!(
                "Failed to rebuild search caches from storage: {}",
                error.message
            );
            s.node_names.clear();
            clear_search_engine(&mut s);
            controller.sidecar_query_cache.lock().clear();
            s.is_indexing = false;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::lexical::exact_symbol_merged_lexical_queries;
    use codestory_contracts::graph::{
        Edge, EdgeId, EdgeKind, Node, NodeId as CoreNodeId, NodeKind, Occurrence, OccurrenceKind,
        ResolutionCertainty, SourceLocation,
    };
    use crossbeam_channel::unbounded;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex as StdMutex, MutexGuard as StdMutexGuard};
    use tempfile::tempdir;

    static ENV_TEST_LOCK: StdMutex<()> = StdMutex::new(());

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(value) = self.previous.as_deref() {
                    std::env::set_var(self.key, value);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    fn assert_mandatory_sidecar_unavailable(error: &ApiError) {
        assert_eq!(error.code, "retrieval_unavailable");
        assert!(
            error
                .message
                .contains("sidecar retrieval primary is unavailable or degraded")
                || error
                    .message
                    .contains("sidecar retrieval primary is mandatory"),
            "expected mandatory sidecar failure, got {error:?}"
        );
        let details = error.details.as_ref().expect("retrieval error details");
        assert_eq!(details.failed_layer.as_deref(), Some("retrieval_sidecar"));
        assert!(
            !details.next_commands.is_empty(),
            "retrieval error should include repair commands: {error:?}"
        );
    }

    #[test]
    fn indexable_source_path_tracks_indexer_structural_and_template_surfaces() {
        for relative_path in [
            "src/lib.rs",
            "src/main.go",
            "src/App.vue",
            "src/App.svelte",
            "src/pages/index.astro",
            "public/index.html",
            "public/site.css",
            "db/schema.sql",
        ] {
            assert!(
                indexable_source_path(Path::new(relative_path)),
                "runtime freshness should count indexer-indexable path: {relative_path}"
            );
        }
    }

    #[test]
    fn indexable_source_path_keeps_non_code_data_outside_freshness_gate() {
        assert!(
            !indexable_source_path(Path::new("target/run-output.log")),
            "runtime freshness should not count unsupported output artifacts"
        );
    }

    struct HybridTestEnv {
        guards: Vec<EnvGuard>,
        _lock: StdMutexGuard<'static, ()>,
    }

    impl HybridTestEnv {
        fn push(&mut self, guard: EnvGuard) {
            self.guards.push(guard);
        }
    }

    fn hybrid_test_env() -> HybridTestEnv {
        let lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        HybridTestEnv {
            guards: vec![
                EnvGuard::set(HYBRID_RETRIEVAL_ENABLED_ENV, "true"),
                EnvGuard::set(EMBEDDING_RUNTIME_MODE_ENV, "hash"),
                EnvGuard::set(EMBEDDING_PROFILE_ENV, "bge-small-en-v1.5"),
                EnvGuard::remove(EMBEDDING_MODEL_ID_ENV),
                EnvGuard::remove(EMBEDDING_POOLING_ENV),
                EnvGuard::remove(EMBEDDING_QUERY_PREFIX_ENV),
                EnvGuard::remove(EMBEDDING_DOCUMENT_PREFIX_ENV),
                EnvGuard::remove(EMBEDDING_LAYER_NORM_ENV),
                EnvGuard::remove(EMBEDDING_TRUNCATE_DIM_ENV),
                EnvGuard::remove(EMBEDDING_EXPECTED_DIM_ENV),
                EnvGuard::remove(SEMANTIC_DOC_SCOPE_ENV),
                EnvGuard::remove(SEMANTIC_DOC_ALIAS_MODE_ENV),
                EnvGuard::remove(SEMANTIC_DOC_MAX_TOKENS_ENV),
                EnvGuard::remove(SEMANTIC_STREAM_PENDING_DOCS_ENV),
                EnvGuard::remove(SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV),
            ],
            _lock: lock,
        }
    }

    #[test]
    fn graph_edge_dto_defaults_structural_member_certainty() {
        let flags = AppGraphFeatureFlags {
            include_edge_certainty: true,
            include_callsite_identity: true,
            include_candidate_targets: true,
        };

        let member = graph_edge_dto(
            Edge {
                id: EdgeId(1),
                source: CoreNodeId(10),
                target: CoreNodeId(20),
                kind: EdgeKind::MEMBER,
                ..Default::default()
            },
            flags,
        );
        let unresolved_call = graph_edge_dto(
            Edge {
                id: EdgeId(2),
                source: CoreNodeId(10),
                target: CoreNodeId(30),
                kind: EdgeKind::CALL,
                ..Default::default()
            },
            flags,
        );
        let explicit_probable = graph_edge_dto(
            Edge {
                id: EdgeId(3),
                source: CoreNodeId(10),
                target: CoreNodeId(40),
                kind: EdgeKind::MEMBER,
                certainty: Some(ResolutionCertainty::Probable),
                ..Default::default()
            },
            flags,
        );

        assert_eq!(member.certainty.as_deref(), Some("certain"));
        assert_eq!(unresolved_call.certainty, None);
        assert_eq!(explicit_probable.certainty.as_deref(), Some("probable"));
    }

    #[test]
    fn parse_search_intent_query_extracts_supported_filters() {
        let parsed = parse_search_intent_query(
            "kind:function name:`listUsers` path:src/routes.ts lang:typescript",
        );

        assert_eq!(parsed.effective_query, "listUsers");
        assert_eq!(
            parsed.filters,
            vec![
                SearchIntentFilter::Kind("function".to_string()),
                SearchIntentFilter::Name("listUsers".to_string()),
                SearchIntentFilter::Path("src/routes.ts".to_string()),
                SearchIntentFilter::Language("typescript".to_string()),
            ]
        );

        let unknown_prefix = parse_search_intent_query("owner:web /api/users");
        assert_eq!(unknown_prefix.effective_query, "owner:web /api/users");
        assert!(unknown_prefix.filters.is_empty());
    }

    #[test]
    fn search_intent_filters_hits_by_kind_path_name_and_language() {
        fn hit(
            id: &str,
            display_name: &str,
            kind: codestory_contracts::api::NodeKind,
            file_path: &str,
        ) -> SearchHit {
            SearchHit {
                node_id: codestory_contracts::api::NodeId(id.to_string()),
                display_name: display_name.to_string(),
                kind,
                file_path: Some(file_path.to_string()),
                line: Some(1),
                score: 1.0,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: None,
                score_breakdown: None,
            }
        }

        let mut hits = vec![
            hit(
                "a",
                "listUsers",
                codestory_contracts::api::NodeKind::FUNCTION,
                "src/routes.ts",
            ),
            hit(
                "b",
                "Users",
                codestory_contracts::api::NodeKind::STRUCT,
                "src/routes.ts",
            ),
            hit(
                "c",
                "listUsers",
                codestory_contracts::api::NodeKind::FUNCTION,
                "src/routes.rs",
            ),
        ];

        apply_search_intent_filters(
            &mut hits,
            &[
                SearchIntentFilter::Kind("function".to_string()),
                SearchIntentFilter::Path("routes.ts".to_string()),
                SearchIntentFilter::Name("listUsers".to_string()),
                SearchIntentFilter::Language("typescript".to_string()),
            ],
        );

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].display_name, "listUsers");
        assert_eq!(hits[0].file_path.as_deref(), Some("src/routes.ts"));
    }

    #[test]
    fn language_filter_uses_shared_registry_extensions() {
        for (requested, path) in [
            ("bash", "scripts/bootstrap.sh"),
            ("bash", "scripts/bootstrap.bash"),
            ("sh", "scripts/bootstrap.sh"),
            ("python", "pkg/types.pyi"),
            ("ts", "src/server.mts"),
            ("typescript", "src/server.cts"),
            ("dart", "lib/main.dart"),
            ("html", "templates/index.htm"),
            ("css", "assets/site.css"),
            ("sql", "db/schema.sql"),
            ("c++", "include/runtime.hh"),
            ("c#", "src/App.cs"),
            ("markdown", "docs/guide.mdx"),
        ] {
            assert!(
                language_filter_matches_path(requested, path),
                "expected language:{requested} to match {path}"
            );
        }

        assert!(!language_filter_matches_path("bash", "src/main.py"));
        assert!(!language_filter_matches_path(
            "sh",
            "scripts/bootstrap.bash"
        ));
        assert!(!language_filter_matches_path("tsx", "src/server.ts"));
        assert!(!language_filter_matches_path("jsx", "src/app.js"));

        assert!(indexed_file_matches_language_filter(
            "typescript",
            Path::new("src/Widget.tsx"),
            "tsx"
        ));
        assert!(indexed_file_matches_language_filter(
            "bash",
            Path::new("scripts/bootstrap.sh"),
            "bash"
        ));
        assert!(!indexed_file_matches_language_filter(
            "typescript",
            Path::new("src/server.ts"),
            "tsx"
        ));
    }

    #[test]
    fn llm_doc_embed_batch_size_uses_throughput_default() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::remove(LLM_DOC_EMBED_BATCH_SIZE_ENV);

        assert_eq!(llm_doc_embed_batch_size(), 128);
    }

    #[test]
    fn framework_route_coverage_matrix_lists_coverage_evidence_and_known_gaps() {
        let coverage = framework_route_coverage_matrix();
        let frameworks = coverage
            .iter()
            .map(|entry| entry.framework.as_str())
            .collect::<HashSet<_>>();
        for expected in [
            "express",
            "react-router",
            "sveltekit",
            "nextjs",
            "remix",
            "astro",
            "nuxt",
            "fastify",
            "koa",
            "hono",
            "nestjs",
            "django",
            "flask",
            "fastapi",
            "rails",
            "laravel",
            "spring",
            "aspnet",
            "axum",
            "actix",
            "rocket",
            "gin",
            "chi",
            "echo",
            "fiber",
            "vue-router",
        ] {
            assert!(
                frameworks.contains(expected),
                "coverage matrix missing {expected}"
            );
        }
        assert!(coverage.iter().all(|entry| {
            !entry.coverage_evidence.is_empty()
                && !entry.confidence_floor.is_empty()
                && !entry.handler_link_support.is_empty()
                && !entry.unsupported_patterns.is_empty()
                && !entry.known_gaps.is_empty()
        }));
        assert!(
            coverage
                .iter()
                .filter(|entry| entry.language == "go")
                .all(|entry| entry.handler_link_support == "not_claimed_text_only")
        );
    }

    #[test]
    fn llm_doc_embed_batch_size_allows_wider_managed_batches() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::set(LLM_DOC_EMBED_BATCH_SIZE_ENV, "1024");

        assert_eq!(llm_doc_embed_batch_size(), 1024);
    }

    #[test]
    fn stream_pending_llm_symbol_docs_defaults_to_enabled() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::remove(SEMANTIC_STREAM_PENDING_DOCS_ENV);
        assert!(stream_pending_llm_symbol_docs_from_env());

        let _env = EnvGuard::set(SEMANTIC_STREAM_PENDING_DOCS_ENV, "false");
        assert!(!stream_pending_llm_symbol_docs_from_env());
    }

    #[test]
    fn semantic_stream_sort_window_defaults_to_one_batch() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::remove(SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV);
        assert_eq!(semantic_stream_sort_window_batches_from_env(), 1);

        let _env = EnvGuard::set(SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV, "1");
        assert_eq!(semantic_stream_sort_window_batches_from_env(), 1);

        let _env = EnvGuard::set(SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV, "999");
        assert_eq!(semantic_stream_sort_window_batches_from_env(), 16);
    }

    #[test]
    fn semantic_doc_scope_defaults_to_durable_symbols_and_all_scope_is_opt_in() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::remove(SEMANTIC_DOC_SCOPE_ENV);
        assert_eq!(
            semantic_doc_scope_from_env(),
            SemanticDocScope::DurableSymbols
        );
        assert_eq!(
            semantic_doc_scope_from_value("all"),
            SemanticDocScope::AllSymbols
        );
        assert_eq!(
            semantic_doc_scope_from_value("full"),
            SemanticDocScope::AllSymbols
        );

        assert!(llm_indexable_kind(NodeKind::FUNCTION));
        assert!(llm_indexable_kind(NodeKind::STRUCT));
        assert!(llm_indexable_kind(NodeKind::GLOBAL_VARIABLE));
        assert!(llm_indexable_kind(NodeKind::CONSTANT));
        assert!(!llm_indexable_kind(NodeKind::MODULE));
        assert!(!llm_indexable_kind(NodeKind::FIELD));
        assert!(!llm_indexable_kind(NodeKind::VARIABLE));

        assert!(llm_indexable_kind_for_scope(
            NodeKind::MODULE,
            SemanticDocScope::AllSymbols
        ));
        assert!(llm_indexable_kind_for_scope(
            NodeKind::FIELD,
            SemanticDocScope::AllSymbols
        ));
        assert!(llm_indexable_kind_for_scope(
            NodeKind::VARIABLE,
            SemanticDocScope::AllSymbols
        ));
        assert!(!llm_indexable_kind_for_scope(
            NodeKind::FILE,
            SemanticDocScope::AllSymbols
        ));
        assert!(!llm_indexable_kind_for_scope(
            NodeKind::UNKNOWN,
            SemanticDocScope::AllSymbols
        ));
        assert!(!llm_indexable_kind_for_scope(
            NodeKind::BUILTIN_TYPE,
            SemanticDocScope::AllSymbols
        ));
    }

    #[test]
    fn semantic_doc_alias_mode_defaults_to_alias_variant() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::remove(SEMANTIC_DOC_ALIAS_MODE_ENV);
        assert_eq!(
            semantic_doc_alias_mode_from_env(),
            SemanticDocAliasMode::AliasVariant
        );
        assert_eq!(
            semantic_doc_alias_mode_from_value("current_alias"),
            SemanticDocAliasMode::CurrentAlias
        );
        assert_eq!(
            semantic_doc_alias_mode_from_value("no_alias"),
            SemanticDocAliasMode::NoAlias
        );
        assert_eq!(
            semantic_doc_alias_mode_from_value("compact"),
            SemanticDocAliasMode::AliasVariant
        );
    }

    #[test]
    fn semantic_doc_token_budget_defaults_to_safe_window() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::remove(SEMANTIC_DOC_MAX_TOKENS_ENV);

        assert_eq!(
            semantic_doc_max_tokens_from_env(),
            SEMANTIC_DOC_DEFAULT_MAX_TOKENS
        );
        assert!(semantic_doc_shape_contract().contains("max_tokens=128"));
    }

    fn pending_semantic_doc_for_test(node_id: i64, doc_text: &str) -> PendingLlmSymbolDoc {
        PendingLlmSymbolDoc {
            node_id: CoreNodeId(node_id),
            file_node_id: Some(CoreNodeId(1)),
            kind: NodeKind::FUNCTION,
            display_name: format!("doc_{node_id}"),
            qualified_name: None,
            file_path: None,
            start_line: None,
            doc_text: doc_text.to_string(),
            doc_hash: llm_symbol_doc_hash(doc_text),
            dense_reason: DenseAnchorReason::PublicApi,
        }
    }

    fn semantic_policy_node(id: i64, kind: NodeKind, name: &str, file_id: i64) -> Node {
        Node {
            id: CoreNodeId(id),
            kind,
            serialized_name: name.to_string(),
            qualified_name: Some(format!("pkg::{name}")),
            file_node_id: Some(CoreNodeId(file_id)),
            start_line: Some(1),
            end_line: Some(3),
            ..Default::default()
        }
    }

    fn semantic_policy_context(path: &str, node_id: CoreNodeId) -> SemanticDocGraphContext {
        let mut context = SemanticDocGraphContext::default();
        context.file_paths.insert(node_id, path.to_string());
        context
    }

    #[test]
    fn dense_policy_skips_private_trivial_helpers() {
        let node = semantic_policy_node(10, NodeKind::FUNCTION, "helper", 1);
        let context = semantic_policy_context("src/internal/helper.rs", node.id);

        let reason = dense_anchor_reason_for_node(
            &context,
            &node,
            "helper",
            Some("src/internal/helper.rs"),
            "semantic_doc_version: 4\nsymbol: helper\nkind: FUNCTION\n",
            Some(AccessKind::Private),
        );

        assert_eq!(reason, None);
    }

    #[test]
    fn dense_policy_does_not_treat_every_handler_name_as_entrypoint() {
        let node = semantic_policy_node(14, NodeKind::FUNCTION, "handler", 1);
        let context = semantic_policy_context("src/internal/request.rs", node.id);

        let reason = dense_anchor_reason_for_node(
            &context,
            &node,
            "handler",
            Some("src/internal/request.rs"),
            "semantic_doc_version: 4\nsymbol: handler\nkind: FUNCTION\n",
            Some(AccessKind::Private),
        );

        assert_eq!(reason, None);
    }

    #[test]
    fn dense_policy_only_embeds_high_signal_central_nodes() {
        let ordinary = semantic_policy_node(15, NodeKind::FUNCTION, "ordinary", 1);
        let central = semantic_policy_node(16, NodeKind::FUNCTION, "central", 1);
        let mut context = semantic_policy_context("src/internal/graph.rs", ordinary.id);
        context
            .file_paths
            .insert(central.id, "src/internal/graph.rs".to_string());
        context.child_labels.insert(
            ordinary.id,
            ["a", "b", "c", "d"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        );
        context.referenced_labels.insert(
            central.id,
            (0..DENSE_CENTRAL_LABEL_THRESHOLD)
                .map(|index| format!("ref_{index}"))
                .collect(),
        );
        context
            .edge_digests
            .insert(central.id, vec!["CALL=24".to_string()]);

        assert_eq!(
            dense_anchor_reason_for_node(
                &context,
                &ordinary,
                "ordinary",
                Some("src/internal/graph.rs"),
                "semantic_doc_version: 4\nsymbol: ordinary\nkind: FUNCTION\n",
                Some(AccessKind::Private),
            ),
            None
        );
        assert_eq!(
            dense_anchor_reason_for_node(
                &context,
                &central,
                "central",
                Some("src/internal/graph.rs"),
                "semantic_doc_version: 4\nsymbol: central\nkind: FUNCTION\n",
                Some(AccessKind::Private),
            ),
            Some(DenseAnchorReason::CentralGraphNode)
        );
    }

    #[test]
    fn dense_policy_classifies_public_entrypoint_and_documented_symbols() {
        let public_node = semantic_policy_node(11, NodeKind::STRUCT, "ReportBuilder", 1);
        let entrypoint_node = semantic_policy_node(12, NodeKind::FUNCTION, "main", 1);
        let documented_node = semantic_policy_node(13, NodeKind::METHOD, "parse_config", 1);
        let context = semantic_policy_context("src/lib.rs", public_node.id);

        assert_eq!(
            dense_anchor_reason_for_node(
                &context,
                &public_node,
                "ReportBuilder",
                Some("src/lib.rs"),
                "semantic_doc_version: 4\nsymbol: ReportBuilder\nkind: STRUCT\n",
                Some(AccessKind::Public),
            ),
            Some(DenseAnchorReason::PublicApi)
        );
        assert_eq!(
            dense_anchor_reason_for_node(
                &context,
                &entrypoint_node,
                "main",
                Some("src/main.rs"),
                "semantic_doc_version: 4\nsymbol: main\nkind: FUNCTION\n",
                Some(AccessKind::Private),
            ),
            Some(DenseAnchorReason::Entrypoint)
        );
        assert_eq!(
            dense_anchor_reason_for_node(
                &context,
                &documented_node,
                "parse_config",
                Some("src/internal/config.rs"),
                "semantic_doc_version: 4\ncomments: parses user-visible configuration\nbody_summary: validates and normalizes the configuration before runtime startup\n",
                Some(AccessKind::Private),
            ),
            Some(DenseAnchorReason::DocumentedNontrivial)
        );
    }

    #[test]
    fn dense_policy_classifies_cross_language_entrypoints_and_surfaces() {
        let python_app = semantic_policy_node(21, NodeKind::FUNCTION, "app", 1);
        let go_command = semantic_policy_node(22, NodeKind::FUNCTION, "run", 1);
        let csharp_program = semantic_policy_node(23, NodeKind::CLASS, "Program", 1);
        let java_application = semantic_policy_node(24, NodeKind::CLASS, "Application", 1);
        let c_header_api = semantic_policy_node(25, NodeKind::STRUCT, "ClientApi", 1);
        let python_package_api = semantic_policy_node(26, NodeKind::CLASS, "PackageClient", 1);
        let mut context = SemanticDocGraphContext::default();
        context
            .file_paths
            .insert(python_app.id, "service/app.py".to_string());
        context
            .file_paths
            .insert(go_command.id, "cmd/server/main.go".to_string());
        context
            .file_paths
            .insert(csharp_program.id, "src/Program.cs".to_string());
        context.file_paths.insert(
            java_application.id,
            "src/main/java/com/acme/Application.java".to_string(),
        );
        context
            .file_paths
            .insert(c_header_api.id, "include/acme/client_api.hpp".to_string());
        context.file_paths.insert(
            python_package_api.id,
            "packages/acme_sdk/__init__.py".to_string(),
        );

        for (node, display_name, file_path) in [
            (&python_app, "app", "service/app.py"),
            (&go_command, "run", "cmd/server/main.go"),
            (&csharp_program, "Program", "src/Program.cs"),
            (
                &java_application,
                "Application",
                "src/main/java/com/acme/Application.java",
            ),
        ] {
            assert_eq!(
                dense_anchor_reason_for_node(
                    &context,
                    node,
                    display_name,
                    Some(file_path),
                    "semantic_doc_version: 4\nsymbol: entrypoint\nkind: FUNCTION\n",
                    Some(AccessKind::Private),
                ),
                Some(DenseAnchorReason::Entrypoint),
                "{file_path} should classify as an entrypoint"
            );
        }

        for (node, display_name, file_path) in [
            (&c_header_api, "ClientApi", "include/acme/client_api.hpp"),
            (
                &python_package_api,
                "PackageClient",
                "packages/acme_sdk/__init__.py",
            ),
        ] {
            assert_eq!(
                dense_anchor_reason_for_node(
                    &context,
                    node,
                    display_name,
                    Some(file_path),
                    "semantic_doc_version: 4\nsymbol: api\nkind: STRUCT\n",
                    Some(AccessKind::Private),
                ),
                Some(DenseAnchorReason::PublicApi),
                "{file_path} should classify as a public surface"
            );
        }
    }

    #[test]
    fn dense_policy_does_not_embed_plain_public_callables_by_default() {
        let node = semantic_policy_node(17, NodeKind::FUNCTION, "plain_public_function", 1);
        let context = semantic_policy_context("src/lib.rs", node.id);

        let reason = dense_anchor_reason_for_node(
            &context,
            &node,
            "plain_public_function",
            Some("src/lib.rs"),
            "semantic_doc_version: 4\nsymbol: plain_public_function\nkind: FUNCTION\n",
            Some(AccessKind::Public),
        );

        assert_eq!(reason, None);
    }

    #[test]
    fn dense_policy_embeds_package_public_callables_for_dynamic_frameworks() {
        let node = semantic_policy_node(19, NodeKind::FUNCTION, "handle", 1);
        let context = semantic_policy_context("lib/router/index.js", node.id);

        let reason = dense_anchor_reason_for_node(
            &context,
            &node,
            "handle",
            Some("lib/router/index.js"),
            "semantic_doc_version: 4\nsymbol: handle\nkind: FUNCTION\nsignature: function handle(req, res, next) {}\n",
            Some(AccessKind::Private),
        );

        assert_eq!(reason, Some(DenseAnchorReason::PublicApi));

        let windows_node = semantic_policy_node(29, NodeKind::METHOD, "GET /json", 1);
        let windows_path = r"\\?\C:\repo\expressjs-express\lib\response.js";
        let windows_context = semantic_policy_context(windows_path, windows_node.id);

        let windows_reason = dense_anchor_reason_for_node(
            &windows_context,
            &windows_node,
            "GET /json",
            Some(windows_path),
            "semantic_doc_version: 4\nsymbol: GET /json\nkind: METHOD\nsignature: .get('/json')\n",
            Some(AccessKind::Private),
        );

        assert_eq!(windows_reason, Some(DenseAnchorReason::PublicApi));
    }

    #[test]
    fn dense_policy_does_not_embed_comment_only_symbols_by_default() {
        let node = semantic_policy_node(18, NodeKind::FUNCTION, "commented_helper", 1);
        let context = semantic_policy_context("src/internal/helper.rs", node.id);

        let reason = dense_anchor_reason_for_node(
            &context,
            &node,
            "commented_helper",
            Some("src/internal/helper.rs"),
            "semantic_doc_version: 4\ncomments: explains how helper is used by nearby code\nsignature: fn commented_helper() {}\n",
            Some(AccessKind::Private),
        );

        assert_eq!(reason, None);
    }

    #[test]
    fn component_reports_are_extracted_dense_anchors_with_virtual_ids() {
        let node = semantic_policy_node(20, NodeKind::FUNCTION, "central_service", 1);
        let mut context = semantic_policy_context("crates/app/src/service.rs", node.id);
        context
            .edge_digests
            .insert(node.id, vec!["CALL=9".to_string()]);
        let reports = build_component_report_docs(
            &context,
            &[&node],
            &std::collections::HashMap::new(),
            None,
            123,
        );

        assert_eq!(reports.len(), 1);
        let report = &reports[0];
        assert!(report.symbol_doc.node_id.0 < 0);
        assert_eq!(report.symbol_doc.source_provenance, "extracted");
        assert_eq!(report.symbol_doc.policy_version, SEMANTIC_POLICY_VERSION);
        assert!(
            report
                .symbol_doc
                .doc_text
                .contains("component_report: crate:app")
        );
        assert_eq!(
            report.symbol_doc.file_path.as_deref(),
            Some("crates/app/src/service.rs")
        );
        assert!(report.symbol_doc.doc_text.contains("god_nodes:"));
        assert!(report.pending.is_none());
    }

    #[test]
    fn component_reports_group_root_level_source_files() {
        assert_eq!(
            semantic_component_key_for_path(Some("nvm.sh")).as_deref(),
            Some("dir:.")
        );
    }

    #[test]
    fn semantic_graph_context_uses_repo_relative_file_table_paths() {
        let temp = tempdir().expect("create temp dir");
        let storage_path = temp.path().join("codestory.db");
        let mut storage = Storage::open(&storage_path).expect("open storage");
        let verbatim_path = PathBuf::from(r"\\?\C:\work\nvm\nvm.sh");
        storage
            .insert_file(&FileInfo {
                id: 11,
                path: verbatim_path.clone(),
                language: "bash".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 12,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert file");
        let file_node = Node {
            id: CoreNodeId(11),
            kind: NodeKind::FILE,
            serialized_name: verbatim_path.to_string_lossy().to_string(),
            ..Default::default()
        };
        let function_node = Node {
            id: CoreNodeId(101),
            kind: NodeKind::FUNCTION,
            serialized_name: "nvm".to_string(),
            file_node_id: Some(CoreNodeId(11)),
            start_line: Some(1),
            ..Default::default()
        };
        storage
            .insert_nodes_batch(&[file_node.clone(), function_node.clone()])
            .expect("insert nodes");
        let nodes = vec![file_node, function_node.clone()];
        let semantic_nodes = vec![&function_node];
        let context =
            SemanticDocGraphContext::build(&storage, &semantic_nodes, &nodes).expect("context");

        assert_eq!(context.file_path_for_node(&function_node), Some("nvm.sh"));
        assert_eq!(
            context.file_read_path_for_node(&function_node),
            Some("C:/work/nvm/nvm.sh")
        );
        let reports = build_component_report_docs(
            &context,
            &semantic_nodes,
            &std::collections::HashMap::new(),
            None,
            123,
        );
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].symbol_doc.file_path.as_deref(), Some("nvm.sh"));
        assert!(
            reports[0]
                .symbol_doc
                .doc_text
                .contains("component_report: dir:.")
        );
    }

    fn semantic_file_text_cache_node(
        id: i64,
        display_path: &str,
        read_path: &Path,
        context: &mut SemanticDocGraphContext,
    ) -> Node {
        let node = Node {
            id: CoreNodeId(id),
            kind: NodeKind::FUNCTION,
            serialized_name: format!("symbol_{id}"),
            file_node_id: Some(CoreNodeId(id + 100)),
            start_line: Some(1),
            ..Default::default()
        };
        context.file_paths.insert(node.id, display_path.to_string());
        context
            .file_read_paths
            .insert(node.id, read_path.to_string_lossy().to_string());
        node
    }

    #[test]
    fn semantic_file_text_cache_skips_files_above_byte_limit() {
        let temp = tempdir().expect("create temp dir");
        let small_path = temp.path().join("small.rs");
        let large_path = temp.path().join("large.rs");
        fs::write(&small_path, "small").expect("write small file");
        fs::write(&large_path, "too-large").expect("write large file");
        let mut context = SemanticDocGraphContext::default();
        let nodes = [
            semantic_file_text_cache_node(1, "small.rs", &small_path, &mut context),
            semantic_file_text_cache_node(2, "large.rs", &large_path, &mut context),
        ];
        let semantic_nodes = nodes.iter().collect::<Vec<_>>();

        let cache = build_semantic_file_text_cache_with_limits(&context, &semantic_nodes, 5, 100);

        assert_eq!(
            cache
                .get("small.rs")
                .and_then(|contents| contents.as_deref()),
            Some("small")
        );
        assert_eq!(cache.get("large.rs"), Some(&None));
    }

    #[test]
    fn semantic_file_text_cache_respects_aggregate_byte_limit() {
        let temp = tempdir().expect("create temp dir");
        let a_path = temp.path().join("a.rs");
        let b_path = temp.path().join("b.rs");
        let c_path = temp.path().join("c.rs");
        fs::write(&a_path, "aaaa").expect("write a file");
        fs::write(&b_path, "bbbb").expect("write b file");
        fs::write(&c_path, "cc").expect("write c file");
        let mut context = SemanticDocGraphContext::default();
        let nodes = [
            semantic_file_text_cache_node(1, "a.rs", &a_path, &mut context),
            semantic_file_text_cache_node(2, "b.rs", &b_path, &mut context),
            semantic_file_text_cache_node(3, "c.rs", &c_path, &mut context),
        ];
        let semantic_nodes = nodes.iter().collect::<Vec<_>>();

        let cache = build_semantic_file_text_cache_with_limits(&context, &semantic_nodes, 100, 7);

        assert_eq!(
            cache.get("a.rs").and_then(|contents| contents.as_deref()),
            Some("aaaa")
        );
        assert_eq!(cache.get("b.rs"), Some(&None));
        assert_eq!(cache.get("c.rs"), Some(&None));
    }

    fn padded_char_cost(docs: &[PendingLlmSymbolDoc], batch_size: usize) -> usize {
        docs.chunks(batch_size)
            .map(|batch| {
                let max_len = batch
                    .iter()
                    .map(|doc| doc.doc_text.len())
                    .max()
                    .unwrap_or(0);
                max_len * batch.len()
            })
            .sum()
    }

    #[test]
    fn semantic_docs_are_length_bucketed_before_embedding() {
        let mut docs = vec![
            pending_semantic_doc_for_test(1, &"x".repeat(900)),
            pending_semantic_doc_for_test(2, "tiny"),
            pending_semantic_doc_for_test(3, &"m".repeat(880)),
            pending_semantic_doc_for_test(4, "small"),
        ];
        let original_cost = padded_char_cost(&docs, 2);

        sort_pending_llm_symbol_docs_for_embedding_batches(&mut docs);

        assert_eq!(
            docs.iter().map(|doc| doc.node_id.0).collect::<Vec<_>>(),
            vec![2, 4, 3, 1]
        );
        assert!(
            padded_char_cost(&docs, 2) < (original_cost * 3 / 5),
            "length bucketing should avoid padding tiny docs to long-doc batches"
        );
    }

    fn semantic_doc_text_for_test(
        display_name: &str,
        qualified_name: Option<&str>,
        file_path: &str,
        kind: NodeKind,
    ) -> String {
        let node = Node {
            id: CoreNodeId(10),
            kind,
            serialized_name: display_name.to_string(),
            qualified_name: qualified_name.map(str::to_string),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(12),
            ..Default::default()
        };
        let graph_context = SemanticDocGraphContext::default();
        let file_text_cache = HashMap::new();
        build_llm_symbol_doc_text(
            &graph_context,
            &node,
            display_name,
            Some(file_path),
            &file_text_cache,
        )
    }

    #[test]
    fn semantic_doc_text_adds_symbol_aliases_for_supported_language_naming_styles() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "current_alias");
        let _budget = EnvGuard::set(SEMANTIC_DOC_MAX_TOKENS_ENV, "512");
        let cases = [
            (
                "rust",
                "src/game_state.rs",
                "crate::game_state::check_winner",
                Some("crate::game_state::check_winner"),
                "check winner",
                "crate game state check winner",
            ),
            (
                "python",
                "pkg/engine.py",
                "pkg.engine.build_snapshot_digest",
                Some("pkg.engine.build_snapshot_digest"),
                "build snapshot digest",
                "pkg engine build snapshot digest",
            ),
            (
                "javascript",
                "src/GameController.js",
                "GameController.checkWinner",
                Some("GameController.checkWinner"),
                "check winner",
                "game controller check winner",
            ),
            (
                "typescript",
                "src/useWinningMove.ts",
                "useWinningMove",
                None,
                "use winning move",
                "use winning move",
            ),
            (
                "java",
                "src/main/java/GameController.java",
                "com.example.GameController.checkWinner",
                Some("com.example.GameController.checkWinner"),
                "check winner",
                "com example game controller check winner",
            ),
            (
                "c",
                "src/field_ops.c",
                "field_clear_move",
                None,
                "field clear move",
                "field clear move",
            ),
            (
                "cpp",
                "src/field_ops.cpp",
                "Game::Field::clearMove",
                Some("Game::Field::clearMove"),
                "clear move",
                "game field clear move",
            ),
        ];

        for (language, file_path, display_name, qualified_name, terminal_alias, full_alias) in cases
        {
            let doc = semantic_doc_text_for_test(
                display_name,
                qualified_name,
                file_path,
                NodeKind::FUNCTION,
            );
            assert!(
                doc.contains(&format!("language: {language}")),
                "doc should include language for {file_path}:\n{doc}"
            );
            assert!(
                doc.contains(&format!("terminal_alias: {terminal_alias}")),
                "doc should include terminal alias for {display_name}:\n{doc}"
            );
            assert!(
                doc.contains(&format!("name_aliases: {full_alias}")),
                "doc should include normalized full alias for {display_name}:\n{doc}"
            );
        }
    }

    #[test]
    fn semantic_doc_text_adds_kind_role_owner_and_path_alias_context() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "current_alias");
        let _budget = EnvGuard::set(SEMANTIC_DOC_MAX_TOKENS_ENV, "512");
        let doc = semantic_doc_text_for_test(
            "AppController::openProjectWithStoragePath",
            Some("codestory_runtime::AppController::openProjectWithStoragePath"),
            "crates/codestory-runtime/src/lib.rs",
            NodeKind::METHOD,
        );

        assert!(
            doc.contains(
                "symbol_role: method member function object behavior callable routine operation"
            ),
            "method docs should include callable role aliases:\n{doc}"
        );
        assert!(
            doc.contains("owner_aliases: AppController, app controller"),
            "method docs should expose owner/container aliases:\n{doc}"
        );
        assert!(
            doc.contains("terminal_alias: open project with storage path"),
            "method docs should expose normalized terminal names:\n{doc}"
        );
        assert!(
            doc.contains("path_aliases: crates, codestory-runtime, codestory runtime, src, lib"),
            "method docs should expose file path aliases:\n{doc}"
        );
    }

    #[test]
    fn semantic_doc_text_keeps_comments_before_long_file_path() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "current_alias");
        let _budget = EnvGuard::set(SEMANTIC_DOC_MAX_TOKENS_ENV, "128");
        let file_path = r"\\?\C:\Users\alber\AppData\Local\Temp\codestory-search-quality-fixture-with-a-long-path\src\architecture.ts";
        let file_text = r#"// Project source groups create indexing commands and storage access.
export class SourceGroupCxxCdb {
  getIndexerCommands() { return []; }
}
"#;
        let node = Node {
            id: CoreNodeId(10),
            kind: NodeKind::CLASS,
            serialized_name: "SourceGroupCxxCdb".to_string(),
            qualified_name: Some("SourceGroupCxxCdb".to_string()),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(2),
            end_line: Some(4),
            ..Default::default()
        };
        let mut file_text_cache = HashMap::new();
        file_text_cache.insert(file_path.to_string(), Some(file_text.to_string()));

        let doc = build_llm_symbol_doc_text(
            &SemanticDocGraphContext::default(),
            &node,
            "SourceGroupCxxCdb",
            Some(file_path),
            &file_text_cache,
        );

        assert!(
            doc.contains(
                "comments: // Project source groups create indexing commands and storage access."
            ),
            "symbol docs should preserve nearby comments before long file paths consume the token budget:\n{doc}"
        );
    }

    #[test]
    fn semantic_doc_text_alias_modes_are_switchable_for_research() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _budget = EnvGuard::set(SEMANTIC_DOC_MAX_TOKENS_ENV, "512");
        let no_alias = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "no_alias");
        let no_alias_doc = semantic_doc_text_for_test(
            "AppController::openProjectWithStoragePath",
            Some("codestory_runtime::AppController::openProjectWithStoragePath"),
            "crates/codestory-runtime/src/lib.rs",
            NodeKind::METHOD,
        );
        let no_alias_hash = llm_symbol_doc_hash(&no_alias_doc);
        assert!(!no_alias_doc.contains("terminal_alias:"));
        assert!(!no_alias_doc.contains("path_aliases:"));
        drop(no_alias);

        let variant = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "alias_variant");
        let variant_doc = semantic_doc_text_for_test(
            "AppController::openProjectWithStoragePath",
            Some("codestory_runtime::AppController::openProjectWithStoragePath"),
            "crates/codestory-runtime/src/lib.rs",
            NodeKind::METHOD,
        );
        let variant_hash = llm_symbol_doc_hash(&variant_doc);
        assert!(variant_doc.contains("terminal_alias: open project with storage path"));
        assert!(variant_doc.contains("owner_aliases: AppController, app controller"));
        assert!(variant_doc.contains("symbol_role: method member function"));
        assert!(!variant_doc.contains("name_aliases:"));
        assert!(!variant_doc.contains("path_aliases:"));
        assert_ne!(no_alias_hash, variant_hash);
        drop(variant);

        let current = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "current_alias");
        let current_doc = semantic_doc_text_for_test(
            "AppController::openProjectWithStoragePath",
            Some("codestory_runtime::AppController::openProjectWithStoragePath"),
            "crates/codestory-runtime/src/lib.rs",
            NodeKind::METHOD,
        );
        assert!(current_doc.contains("name_aliases:"));
        assert!(current_doc.contains("path_aliases:"));
        assert_ne!(variant_hash, llm_symbol_doc_hash(&current_doc));
        drop(current);
    }

    #[test]
    fn semantic_doc_text_token_budget_respects_configured_limit() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _alias = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "current_alias");
        let _budget = EnvGuard::set(SEMANTIC_DOC_MAX_TOKENS_ENV, "48");
        let doc = semantic_doc_text_for_test(
            "AppController::openProjectWithStoragePath",
            Some("codestory_runtime::AppController::openProjectWithStoragePath"),
            "crates/codestory-runtime/src/lib.rs",
            NodeKind::METHOD,
        );

        assert!(
            semantic_doc_text_budget_cost(&doc) <= 48,
            "budgeted semantic doc should stay within the configured token budget:\n{doc}"
        );
        assert!(
            doc.starts_with("semantic_doc_version:"),
            "budgeted semantic doc should preserve the leading version field:\n{doc}"
        );
        assert!(
            doc.contains("symbol: AppController::openProjectWithStoragePath"),
            "budgeted semantic doc should preserve the symbol identity:\n{doc}"
        );
    }

    #[test]
    fn semantic_doc_text_token_budget_charges_long_identifiers() {
        let doc = concat!(
            "semantic_doc_version: 1\n",
            "symbol: AppController::openProjectWithStoragePath\n",
            "path_aliases: crates codestory runtime src lib rs app controller open project ",
            "storage path AppControllerOpenProjectWithStoragePathRepeatedRepeated\n",
        );
        let truncated = truncate_semantic_doc_text_to_token_budget(doc, 36);

        assert!(
            semantic_doc_text_budget_cost(&truncated) <= 36,
            "budgeted semantic doc should stay under the conservative token proxy:\n{truncated}"
        );
        assert!(
            truncated.split_whitespace().count() < doc.split_whitespace().count(),
            "long identifier-heavy docs should be truncated earlier than whitespace counts alone"
        );
        assert!(
            truncated.contains("symbol: AppController::openProjectWithStoragePath"),
            "budgeted semantic doc should retain leading symbol identity:\n{truncated}"
        );
    }

    fn copy_tictactoe_workspace() -> tempfile::TempDir {
        let temp = tempdir().expect("create temp dir");
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace crates dir")
            .join("codestory-indexer")
            .join("tests")
            .join("fixtures")
            .join("tictactoe");

        for entry in fs::read_dir(&fixtures).expect("read fixtures") {
            let entry = entry.expect("fixture entry");
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let target = temp.path().join(entry.file_name());
            fs::copy(&path, &target).expect("copy fixture");
        }

        temp
    }

    fn write_semantic_fixture(root: &std::path::Path) -> PathBuf {
        let file_path = root.join("semantic_fixture.rs");
        fs::write(
            &file_path,
            r#"
pub fn alpha() {
    beta();
}

pub fn beta() {}
"#,
        )
        .expect("write semantic fixture");
        file_path
    }

    fn write_reindex_semantic_fixture(root: &std::path::Path, digest_text: &str) {
        let src = root.join("src");
        fs::create_dir_all(&src).expect("create src dir");
        let digest_identifier = digest_text.replace(' ', "_");
        fs::write(
            src.join("lib.rs"),
            format!(
                r#"
/// {digest_text}
pub fn build_snapshot_digest({digest_identifier}: &str) -> &'static str {{
    "{digest_text}"
}}

pub fn exact_symbol_anchor() {{}}
"#
            ),
        )
        .expect("write reindex fixture");
    }

    fn insert_semantic_fixture_nodes(storage: &mut Storage, file_path: &std::path::Path) {
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(1),
                    kind: NodeKind::FILE,
                    serialized_name: file_path.to_string_lossy().to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(2),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "alpha".to_string(),
                    qualified_name: Some("pkg::alpha".to_string()),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(2),
                    end_line: Some(4),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(3),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "beta".to_string(),
                    qualified_name: Some("pkg::beta".to_string()),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(6),
                    end_line: Some(6),
                    ..Default::default()
                },
            ])
            .expect("insert semantic fixture nodes");
    }

    #[test]
    fn extract_symbol_search_terms_removes_stopwords_and_short_tokens() {
        let terms = extract_symbol_search_terms("How does the language parsing work in this repo?");
        assert_eq!(terms, vec!["language".to_string(), "parsing".to_string()]);
    }

    #[test]
    fn broad_architecture_search_plan_terms_and_subqueries_are_bounded() {
        let query = "Explain how CodeStory's full-index path flows through CLI/runtime/workspace/indexer/store and how that supports later search, trail, and snippet commands.";
        let terms = search_plan_terms(query);
        for expected in [
            "full-index",
            "full",
            "index",
            "cli",
            "runtime",
            "workspace",
            "indexer",
            "store",
            "search",
            "trail",
            "snippet",
        ] {
            assert!(
                terms
                    .extracted
                    .iter()
                    .any(|term| term.eq_ignore_ascii_case(expected)),
                "expected `{expected}` in extracted terms: {:?}",
                terms.extracted
            );
        }
        assert!(
            terms
                .dropped
                .iter()
                .any(|term| term.term.eq_ignore_ascii_case("explain")),
            "natural-language filler should be visible as dropped terms: {:?}",
            terms.dropped
        );
        let intents = architecture_query_intents(query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        assert!(!intents.is_empty(), "query should have architecture intent");
        let subqueries = search_plan_subqueries(query, &terms, &intents);
        assert!(
            (3..=8).contains(&subqueries.len()),
            "subqueries should be bounded: {subqueries:#?}"
        );
        assert!(
            subqueries.iter().any(|subquery| subquery
                .channels
                .contains(&SearchPlanChannelDto::TypedSymbol)),
            "subqueries should cover typed symbol discovery: {subqueries:#?}"
        );
        assert!(
            subqueries
                .iter()
                .any(|subquery| subquery.channels.contains(&SearchPlanChannelDto::RepoText)),
            "subqueries should cover repo text discovery: {subqueries:#?}"
        );
    }

    #[test]
    fn sourcetrail_style_architecture_prompt_expands_flow_roles() {
        let query = "Explain how Sourcetrail turns project/source-group configuration into indexing work, then how indexed data is accessed by the application. Cite the source files that support the path.";
        let terms = search_plan_terms(query);
        assert!(
            terms
                .dropped
                .iter()
                .any(|term| term.term.eq_ignore_ascii_case("cite")),
            "citation instruction should not become a named anchor: {:?}",
            terms.dropped
        );
        for expected in [
            "BuildIndex",
            "SourceGroup",
            "IndexerCommand",
            "build",
            "index",
            "storage",
            "persistence",
        ] {
            assert!(
                terms
                    .extracted
                    .iter()
                    .any(|term| term.eq_ignore_ascii_case(expected)),
                "expected inferred architecture term `{expected}` in {:?}",
                terms.extracted
            );
        }

        let intents = architecture_query_intents(query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        assert!(!intents.is_empty(), "query should have architecture intent");

        let subqueries = search_plan_subqueries(query, &terms, &intents);
        assert!(
            !subqueries
                .iter()
                .any(|subquery| subquery.role == "named_anchor" && subquery.query == "Cite"),
            "generic citation wording should not consume a named-anchor slot: {subqueries:#?}"
        );
        for expected_role in [
            "build_index_entrypoint",
            "source_group_configuration",
            "indexing_work",
            "storage_access_surface",
        ] {
            assert!(
                subqueries
                    .iter()
                    .any(|subquery| subquery.role == expected_role),
                "expected role subquery `{expected_role}` in {subqueries:#?}"
            );
        }
        let typed_anchor_terms = subqueries
            .iter()
            .find(|subquery| subquery.role == "typed_anchor_terms")
            .map(|subquery| subquery.query.as_str())
            .expect("typed anchor terms");
        for expected in ["BuildIndex", "SourceGroup", "IndexerCommand"] {
            assert!(
                typed_anchor_terms.contains(expected),
                "typed anchor terms should contain `{expected}`, got `{typed_anchor_terms}`"
            );
        }
    }

    #[test]
    fn event_output_architecture_prompt_expands_processor_abstraction() {
        let query = "Explain how codex exec --json flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output.";
        let terms = search_plan_terms(query);
        assert!(
            terms.extracted.iter().any(|term| term == "EventProcessor"),
            "event-output architecture prompt should infer source-truth abstraction: {:?}",
            terms.extracted
        );

        let intents = architecture_query_intents(query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        assert!(!intents.is_empty(), "query should have architecture intent");

        let subqueries = search_plan_subqueries(query, &terms, &intents);
        let typed_anchor_terms = subqueries
            .iter()
            .find(|subquery| subquery.role == "typed_anchor_terms")
            .map(|subquery| subquery.query.as_str())
            .expect("typed anchor terms");
        assert!(
            typed_anchor_terms.contains("EventProcessor"),
            "typed anchor terms should include EventProcessor, got `{typed_anchor_terms}`"
        );
    }

    #[test]
    fn multi_anchor_agent_question_prioritizes_named_anchor_subquery_terms() {
        let query = "Explain how ProjectAlpha turns configuration into processing work, then how processed data is accessed by the application. Anchor the answer around ConfigGroup, WorkerRunner, and DataAccess.";
        let intents = architecture_query_intents(query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        assert!(
            intents.iter().any(|intent| intent == "orchestration"),
            "explain-how architecture question should trigger a search plan: {intents:#?}"
        );
        let terms = search_plan_terms(query);
        for expected in ["ConfigGroup", "WorkerRunner", "DataAccess"] {
            assert!(
                terms.extracted.iter().any(|term| term == expected),
                "expected named anchor `{expected}` in extracted terms: {:?}",
                terms.extracted
            );
        }

        let subqueries = search_plan_subqueries(query, &terms, &intents);
        let typed_anchor_terms = subqueries
            .iter()
            .find(|subquery| subquery.role == "typed_anchor_terms")
            .map(|subquery| subquery.query.as_str())
            .expect("typed anchor subquery");
        for expected in ["ConfigGroup", "WorkerRunner", "DataAccess"] {
            assert!(
                subqueries
                    .iter()
                    .any(|subquery| subquery.role == "named_anchor" && subquery.query == expected),
                "expected named-anchor subquery for `{expected}`: {subqueries:#?}"
            );
            assert!(
                typed_anchor_terms.contains(expected),
                "typed anchor subquery should prioritize named anchors; got `{typed_anchor_terms}`"
            );
        }
    }

    #[test]
    fn search_plan_still_runs_for_seed_anchor_drill_queries_with_exact_hits() {
        let query = "Explain how a full indexing run moves through the runtime. Seed anchors: run_index, RuntimeContext::ensure_open_from_summary, WorkspaceIndexer::run";
        let intents = architecture_query_intents(query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        assert!(!intents.is_empty(), "query should have architecture intent");

        assert!(
            search_plan_eligible(query, 3, &intents),
            "drill seed-anchor queries need a plan even when the anchors produce exact symbol hits"
        );

        let same_query_without_seed_anchors = "Explain how run_index RuntimeContext::ensure_open_from_summary WorkspaceIndexer::run moves through the runtime.";
        assert!(
            !search_plan_eligible(same_query_without_seed_anchors, 3, &intents),
            "ordinary exact-symbol queries should keep the exact-hit suppression"
        );
    }

    #[test]
    fn broad_explain_how_search_plan_survives_generic_exact_hits() {
        let query = "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.";
        let intents = architecture_query_intents(query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        assert!(!intents.is_empty(), "query should have architecture intent");

        assert!(
            search_plan_eligible(query, 7, &intents),
            "generic exact hits such as CLI should not suppress broad architecture search plans"
        );
        let terms = search_plan_terms(query);
        let roles = search_plan_subqueries(query, &terms, &intents)
            .into_iter()
            .map(|subquery| subquery.role)
            .collect::<Vec<_>>();
        for expected in [
            "workspace_discovery",
            "symbol_extraction",
            "persistence_surface",
        ] {
            assert!(
                roles.iter().any(|role| role == expected),
                "broad explain-how prompt should expand architecture role `{expected}`: {roles:#?}"
            );
        }

        let ordinary_exact_query =
            "Explain how run_index RuntimeContext::ensure_open_from_summary moves through runtime.";
        assert!(
            !search_plan_eligible(ordinary_exact_query, 2, &intents),
            "ordinary exact-symbol explanations should still stay exact-first unless they name enough architecture surfaces"
        );
    }

    #[test]
    fn search_plan_preserves_seed_anchor_line_exactly() {
        let query = "Explain how a full indexing run moves through the runtime. Seed anchors: run_index, run_index_once, RuntimeContext::ensure_open_from_summary, IndexService::run_indexing_blocking, AppController::run_indexing_blocking_inner, index_incremental, WorkspaceManifest::build_execution_plan, WorkspaceIndexer::run, WorkspaceIndexer::flush_projection_batch";
        let intents = architecture_query_intents(query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        assert!(!intents.is_empty(), "query should have architecture intent");

        let terms = search_plan_terms(query);
        let subqueries = search_plan_subqueries(query, &terms, &intents);
        for expected in [
            "run_index",
            "run_index_once",
            "RuntimeContext::ensure_open_from_summary",
            "IndexService::run_indexing_blocking",
            "AppController::run_indexing_blocking_inner",
            "index_incremental",
            "WorkspaceManifest::build_execution_plan",
            "WorkspaceIndexer::run",
            "WorkspaceIndexer::flush_projection_batch",
        ] {
            assert!(
                subqueries
                    .iter()
                    .any(|subquery| subquery.role == "named_anchor" && subquery.query == expected),
                "expected exact seed-anchor subquery for `{expected}`: {subqueries:#?}"
            );
        }
    }

    #[test]
    fn public_surface_question_keeps_short_pascal_case_named_anchor() {
        let query = "Explain how public writing/social surfaces connect to Payload collections, comment auth, and the elsewhere feed. Anchor the answer around Posts, getElsewhereFeed, and getCommentAuth.";
        let intents = architecture_query_intents(query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        assert!(!intents.is_empty(), "query should have architecture intent");

        let terms = search_plan_terms(query);
        let subqueries = search_plan_subqueries(query, &terms, &intents);
        for expected in ["Posts", "getElsewhereFeed", "getCommentAuth"] {
            assert!(
                subqueries
                    .iter()
                    .any(|subquery| subquery.role == "named_anchor" && subquery.query == expected),
                "expected named-anchor subquery for `{expected}`: {subqueries:#?}"
            );
        }
    }

    #[test]
    fn payload_content_flow_prompt_expands_source_truth_anchors() {
        let query = "Explain how Root & Runtime public writing and social surfaces connect through Payload collections, post rendering, comment auth/submission, RSS, and the Elsewhere feed. Cite the source files that support the path.";
        let terms = search_plan_terms(query);
        for noisy in ["root", "runtime"] {
            assert!(
                !terms
                    .extracted
                    .iter()
                    .any(|term| term.eq_ignore_ascii_case(noisy)),
                "brand phrase term `{noisy}` should not dominate Payload content-flow search: {:?}",
                terms.extracted
            );
            assert!(
                terms
                    .dropped
                    .iter()
                    .any(|term| term.term.eq_ignore_ascii_case(noisy)
                        && term.reason == "brand_phrase_in_content_flow"),
                "brand phrase term `{noisy}` should be explained as dropped: {:?}",
                terms.dropped
            );
        }
        for expected in [
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
            assert!(
                terms
                    .extracted
                    .iter()
                    .any(|term| term.eq_ignore_ascii_case(expected)),
                "expected Payload content-flow term `{expected}` in {:?}",
                terms.extracted
            );
        }

        let intents = architecture_query_intents(query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        assert!(!intents.is_empty(), "query should have architecture intent");

        let subqueries = search_plan_subqueries(query, &terms, &intents);
        let typed_anchor_terms = subqueries
            .iter()
            .find(|subquery| subquery.role == "typed_anchor_terms")
            .map(|subquery| subquery.query.as_str())
            .expect("typed anchor terms");
        for expected in ["Posts", "Comments", "feed"] {
            assert!(
                typed_anchor_terms.contains(expected),
                "typed anchor terms should include `{expected}`, got `{typed_anchor_terms}`"
            );
        }
        assert!(
            subqueries.iter().any(|subquery| {
                subquery.role == "content_surface"
                    && subquery.query.to_ascii_lowercase().contains("comments")
            }),
            "content role subquery should preserve comment wording: {subqueries:#?}"
        );
        for expected_role in [
            "collection_config_surface",
            "comment_submission_surface",
            "public_feed_surface",
        ] {
            assert!(
                subqueries
                    .iter()
                    .any(|subquery| subquery.role == expected_role),
                "expected role subquery `{expected_role}` in {subqueries:#?}"
            );
        }
        let comment_role_query = subqueries
            .iter()
            .find(|subquery| subquery.role == "comment_submission_surface")
            .map(|subquery| subquery.query.to_ascii_lowercase())
            .expect("comment submission role query");
        for expected in ["comment", "auth", "submission"] {
            assert!(
                comment_role_query.contains(expected),
                "comment role query should contain `{expected}`, got `{comment_role_query}`"
            );
        }
    }

    #[test]
    fn codex_exec_json_prompt_expands_source_truth_anchors() {
        let query = "Explain how `codex exec --json` flows from the top-level CLI into the exec runtime, app-server thread and turn start requests, and JSONL event output. Cite the source files that support the path.";
        let terms = search_plan_terms(query);
        for expected in [
            "EventProcessor",
            "exec cli",
            "exec runtime",
            "exec session",
            "event processor",
            "event output",
            "thread start",
            "turn start",
        ] {
            assert!(
                terms
                    .extracted
                    .iter()
                    .any(|term| term.eq_ignore_ascii_case(expected)),
                "expected Codex exec-flow term `{expected}` in {:?}",
                terms.extracted
            );
        }

        let intents = architecture_query_intents(query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        assert!(!intents.is_empty(), "query should have architecture intent");

        let subqueries = search_plan_subqueries(query, &terms, &intents);
        let typed_anchor_terms = subqueries
            .iter()
            .find(|subquery| subquery.role == "typed_anchor_terms")
            .map(|subquery| subquery.query.as_str())
            .expect("typed anchor terms");
        assert!(
            typed_anchor_terms.contains("EventProcessor"),
            "typed anchor terms should include EventProcessor, got `{typed_anchor_terms}`"
        );
        for expected_role in ["exec_cli_surface", "exec_event_output_surface"] {
            assert!(
                subqueries
                    .iter()
                    .any(|subquery| subquery.role == expected_role),
                "expected role subquery `{expected_role}` in {subqueries:#?}"
            );
        }
        let exec_cli_query = subqueries
            .iter()
            .find(|subquery| subquery.role == "exec_cli_surface")
            .map(|subquery| subquery.query.to_ascii_lowercase())
            .expect("exec CLI role query");
        for expected in ["exec", "cli", "runtime"] {
            assert!(
                exec_cli_query.contains(expected),
                "exec CLI role query should contain `{expected}`, got `{exec_cli_query}`"
            );
        }
        let event_output_query = subqueries
            .iter()
            .find(|subquery| subquery.role == "exec_event_output_surface")
            .map(|subquery| subquery.query.to_ascii_lowercase())
            .expect("event output role query");
        for expected in ["event", "output", "processor"] {
            assert!(
                event_output_query.contains(expected),
                "event-output role query should contain `{expected}`, got `{event_output_query}`"
            );
        }
    }

    fn search_plan_test_hit(
        id: &str,
        display_name: &str,
        file_path: &Path,
        line: u32,
        origin: SearchHitOrigin,
        resolvable: bool,
    ) -> SearchHit {
        SearchHit {
            node_id: NodeId(id.to_string()),
            display_name: display_name.to_string(),
            kind: codestory_contracts::api::NodeKind::METHOD,
            file_path: Some(file_path.to_string_lossy().to_string()),
            line: Some(line),
            score: 1.0,
            origin,
            match_quality: None,
            resolvable,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        }
    }

    #[test]
    fn architecture_repo_text_window_preserves_coverage_surfaces() {
        let query = "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.";
        let mut hits = vec![
            search_plan_test_hit(
                "runtime-lib",
                "crates/codestory-runtime/src/lib.rs",
                Path::new("crates/codestory-runtime/src/lib.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "runtime-agent",
                "crates/codestory-runtime/src/agent/orchestrator.rs",
                Path::new("crates/codestory-runtime/src/agent/orchestrator.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "cli-runtime",
                "crates/codestory-cli/src/runtime.rs",
                Path::new("crates/codestory-cli/src/runtime.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "runtime-semantic",
                "crates/codestory-runtime/src/semantic_doc_text.rs",
                Path::new("crates/codestory-runtime/src/semantic_doc_text.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "runtime-symbol",
                "crates/codestory-runtime/src/symbol_query.rs",
                Path::new("crates/codestory-runtime/src/symbol_query.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "runtime-search",
                "crates/codestory-runtime/src/search/engine.rs",
                Path::new("crates/codestory-runtime/src/search/engine.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "runtime-search-runtime",
                "crates/codestory-runtime/src/search_runtime.rs",
                Path::new("crates/codestory-runtime/src/search_runtime.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "runtime-services",
                "crates/codestory-runtime/src/services.rs",
                Path::new("crates/codestory-runtime/src/services.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "cli-args",
                "crates/codestory-cli/src/args.rs",
                Path::new("crates/codestory-cli/src/args.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "runtime-browser",
                "crates/codestory-runtime/src/browser.rs",
                Path::new("crates/codestory-runtime/src/browser.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "indexer-lib",
                "crates/codestory-indexer/src/lib.rs",
                Path::new("crates/codestory-indexer/src/lib.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "storage-impl",
                "crates/codestory-store/src/storage_impl/mod.rs",
                Path::new("crates/codestory-store/src/storage_impl/mod.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
        ];

        truncate_repo_text_hits_for_query(query, &mut hits, 10);
        let paths = hits
            .iter()
            .filter_map(|hit| hit.file_path.as_deref())
            .collect::<Vec<_>>();

        assert!(paths.contains(&"crates/codestory-runtime/src/lib.rs"));
        assert!(paths.contains(&"crates/codestory-cli/src/runtime.rs"));
        assert!(paths.contains(&"crates/codestory-runtime/src/services.rs"));
        assert!(paths.contains(&"crates/codestory-indexer/src/lib.rs"));
        assert!(paths.contains(&"crates/codestory-store/src/storage_impl/mod.rs"));
        assert_eq!(paths.len(), 10);
    }

    #[test]
    fn architecture_repo_text_window_preserves_non_crate_source_surfaces() {
        let query = "Explain how Sourcetrail turns project/source-group configuration into indexing work, then how indexed data is accessed by the application.";
        let mut hits = vec![
            search_plan_test_hit(
                "custom-command",
                "src/lib/project/SourceGroupCustomCommand.cpp",
                Path::new("src/lib/project/SourceGroupCustomCommand.cpp"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "wizard-data",
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.cpp",
                Path::new(
                    "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.cpp",
                ),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "wizard-info",
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupInfoText.cpp",
                Path::new(
                    "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupInfoText.cpp",
                ),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "factory",
                "src/lib/project/SourceGroupFactory.cpp",
                Path::new("src/lib/project/SourceGroupFactory.cpp"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "factory-custom",
                "src/lib/project/SourceGroupFactoryModuleCustom.cpp",
                Path::new("src/lib/project/SourceGroupFactoryModuleCustom.cpp"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "python-empty",
                "src/lib_python/project/SourceGroupPythonEmpty.cpp",
                Path::new("src/lib_python/project/SourceGroupPythonEmpty.cpp"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "factory-cxx",
                "src/lib_cxx/project/SourceGroupFactoryModuleCxx.cpp",
                Path::new("src/lib_cxx/project/SourceGroupFactoryModuleCxx.cpp"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "wizard-data-h",
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.h",
                Path::new(
                    "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.h",
                ),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "wizard-info-h",
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupInfoText.h",
                Path::new(
                    "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupInfoText.h",
                ),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "factory-java",
                "src/lib_java/project/SourceGroupFactoryModuleJava.cpp",
                Path::new("src/lib_java/project/SourceGroupFactoryModuleJava.cpp"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "cdb",
                "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
                Path::new("src/lib_cxx/project/SourceGroupCxxCdb.cpp"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "storage-access",
                "src/lib/data/storage/StorageAccess.h",
                Path::new("src/lib/data/storage/StorageAccess.h"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "storage-proxy",
                "src/lib/data/storage/StorageAccessProxy.cpp",
                Path::new("src/lib/data/storage/StorageAccessProxy.cpp"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
        ];

        truncate_repo_text_hits_for_query(query, &mut hits, 10);
        let paths = hits
            .iter()
            .filter_map(|hit| hit.file_path.as_deref())
            .collect::<Vec<_>>();

        assert!(paths.contains(&"src/lib_cxx/project/SourceGroupCxxCdb.cpp"));
        assert!(paths.contains(&"src/lib/data/storage/StorageAccess.h"));
        assert!(paths.contains(&"src/lib/data/storage/StorageAccessProxy.cpp"));
        assert_eq!(paths.len(), 10);
    }

    #[test]
    fn architecture_cross_source_coverage_promotes_concrete_role_representatives() {
        let query = "Explain how Sourcetrail turns project/source-group configuration into indexing work, then how indexed data is accessed by the application.";
        let mut indexed_hits = vec![
            search_plan_test_hit(
                "persistent-h",
                "StorageAccess",
                Path::new("src/lib/data/storage/PersistentStorage.h"),
                17,
                SearchHitOrigin::IndexedSymbol,
                true,
            ),
            search_plan_test_hit(
                "generic-indexer",
                "Indexer",
                Path::new("src/lib/data/indexer/Indexer.h"),
                1,
                SearchHitOrigin::IndexedSymbol,
                true,
            ),
            search_plan_test_hit(
                "persistent-cpp",
                "PersistentStorage::PersistentStorage",
                Path::new("src/lib/data/storage/PersistentStorage.cpp"),
                32,
                SearchHitOrigin::IndexedSymbol,
                true,
            ),
            search_plan_test_hit(
                "project",
                "Project::isIndexing",
                Path::new("src/lib/project/Project.cpp"),
                92,
                SearchHitOrigin::IndexedSymbol,
                true,
            ),
        ];
        for index in 0..6 {
            indexed_hits.push(search_plan_test_hit(
                &format!("generic-indexer-{index}"),
                "Indexer",
                Path::new(&format!("src/lib/data/indexer/Indexer{index}.h")),
                1,
                SearchHitOrigin::IndexedSymbol,
                true,
            ));
        }
        let mut indexed_candidates = indexed_hits.clone();
        indexed_candidates.push(search_plan_test_hit(
            "storage-access-h",
            "StorageAccess::~StorageAccess",
            Path::new("src/lib/data/storage/StorageAccess.h"),
            36,
            SearchHitOrigin::IndexedSymbol,
            true,
        ));

        let mut repo_text_hits = vec![search_plan_test_hit(
            "cdb-h",
            "src/lib_cxx/project/SourceGroupCxxCdb.h",
            Path::new("src/lib_cxx/project/SourceGroupCxxCdb.h"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        )];
        for index in 0..9 {
            repo_text_hits.push(search_plan_test_hit(
                &format!("wizard-{index}"),
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.cpp",
                Path::new(&format!(
                    "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData{index}.cpp"
                )),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ));
        }
        let mut repo_text_candidates = repo_text_hits.clone();
        repo_text_candidates.push(search_plan_test_hit(
            "indexer-java",
            "src/lib_java/data/indexer/IndexerJava.cpp",
            Path::new("src/lib_java/data/indexer/IndexerJava.cpp"),
            15,
            SearchHitOrigin::TextMatch,
            false,
        ));

        apply_architecture_cross_source_coverage(
            query,
            &mut indexed_hits,
            &mut repo_text_hits,
            &indexed_candidates,
            &repo_text_candidates,
            10,
        );

        let indexed_paths = indexed_hits
            .iter()
            .filter_map(|hit| hit.file_path.as_deref())
            .collect::<Vec<_>>();
        let repo_text_paths = repo_text_hits
            .iter()
            .filter_map(|hit| hit.file_path.as_deref())
            .collect::<Vec<_>>();

        for expected in [
            "src/lib/project/Project.cpp",
            "src/lib/data/storage/PersistentStorage.cpp",
            "src/lib/data/storage/PersistentStorage.h",
            "src/lib/data/storage/StorageAccess.h",
        ] {
            assert!(
                indexed_paths.contains(&expected),
                "expected indexed path `{expected}` in {indexed_paths:#?}"
            );
        }
        for expected in [
            "src/lib_cxx/project/SourceGroupCxxCdb.h",
            "src/lib_java/data/indexer/IndexerJava.cpp",
        ] {
            assert!(
                repo_text_paths.contains(&expected),
                "expected repo-text path `{expected}` in {repo_text_paths:#?}"
            );
        }
        assert_eq!(indexed_hits.len(), 10);
        assert_eq!(repo_text_hits.len(), 10);
    }

    #[test]
    fn architecture_cross_source_coverage_uses_replacement_budget_for_actual_admissions() {
        let query = "Explain how Sourcetrail turns project/source-group configuration into indexing work, then how indexed data is accessed by the application.";
        let mut indexed_hits = Vec::new();
        let indexed_candidates = Vec::new();
        let mut repo_text_hits = (0..10)
            .map(|index| {
                search_plan_test_hit(
                    &format!("generic-source-group-{index}"),
                    &format!("src/lib/project/SourceGroupGeneric{index}.cpp"),
                    Path::new(&format!("src/lib/project/SourceGroupGeneric{index}.cpp")),
                    1,
                    SearchHitOrigin::TextMatch,
                    false,
                )
            })
            .collect::<Vec<_>>();
        let mut repo_text_candidates = repo_text_hits.clone();
        for (id, path) in [
            (
                "source-group-cdb-h",
                "src/lib_cxx/project/SourceGroupCxxCdb.h",
            ),
            (
                "source-group-cdb-cpp",
                "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
            ),
            (
                "indexer-command-cxx-cpp",
                "src/lib_cxx/data/indexer/IndexerCommandCxx.cpp",
            ),
            (
                "indexer-command-cxx-h",
                "src/lib_cxx/data/indexer/IndexerCommandCxx.h",
            ),
            ("indexer-java", "src/lib_java/data/indexer/IndexerJava.cpp"),
            (
                "storage-proxy",
                "src/lib/data/storage/StorageAccessProxy.cpp",
            ),
        ] {
            repo_text_candidates.push(search_plan_test_hit(
                id,
                path,
                Path::new(path),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ));
        }

        apply_architecture_cross_source_coverage(
            query,
            &mut indexed_hits,
            &mut repo_text_hits,
            &indexed_candidates,
            &repo_text_candidates,
            10,
        );

        let repo_text_paths = repo_text_hits
            .iter()
            .filter_map(|hit| hit.file_path.as_deref())
            .collect::<Vec<_>>();
        for expected in [
            "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
            "src/lib_java/data/indexer/IndexerJava.cpp",
            "src/lib/data/storage/StorageAccessProxy.cpp",
        ] {
            assert!(
                repo_text_paths.contains(&expected),
                "expected high-coverage late candidate `{expected}` in {repo_text_paths:#?}"
            );
        }
        assert_eq!(repo_text_paths.len(), 10);
    }

    #[test]
    fn search_plan_rejected_hits_exposes_repo_text_coverage_candidates() {
        let chosen = search_plan_test_hit(
            "project",
            "Project::isIndexing",
            Path::new("src/lib/project/Project.cpp"),
            92,
            SearchHitOrigin::IndexedSymbol,
            true,
        );
        let anchor_groups = vec![SearchPlanAnchorGroupDto {
            anchor: "Project::isIndexing".to_string(),
            chosen_symbol: Some(chosen),
            supporting_hits: Vec::new(),
            promotion_status: SearchPlanPromotionStatusDto::TypedAnchor,
            promotion_method: None,
            caller_count: 0,
            definition_only: false,
            no_visible_callers: false,
            confidence: "high".to_string(),
            reasons: Vec::new(),
        }];
        let indexed_hits = vec![search_plan_test_hit(
            "storage-access",
            "StorageAccess::~StorageAccess",
            Path::new("src/lib/data/storage/StorageAccess.h"),
            36,
            SearchHitOrigin::IndexedSymbol,
            true,
        )];
        let repo_text_hits = vec![search_plan_test_hit(
            "source-group-cdb",
            "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
            Path::new("src/lib_cxx/project/SourceGroupCxxCdb.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        )];

        let rejected =
            search_plan_rejected_hits(&anchor_groups, &[], &indexed_hits, &repo_text_hits);

        let repo_text = rejected
            .iter()
            .find(|hit| hit.origin == SearchHitOrigin::TextMatch)
            .expect("repo-text rejected hit should be retained for diagnostics");
        assert_eq!(
            repo_text.file_path.as_deref(),
            Some("src/lib_cxx/project/SourceGroupCxxCdb.cpp")
        );
        assert!(
            repo_text.reason.contains("source=repo_text")
                && repo_text
                    .reason
                    .contains("coverage_key=source_group:configuration:impl")
                && repo_text.reason.contains("coverage_score=10"),
            "repo-text rejection reason should include coverage provenance: {repo_text:#?}"
        );
    }

    #[test]
    fn architecture_coverage_promotes_exec_flow_source_surfaces() {
        let expected = [
            (
                "codex-rs/cli/src/main.rs",
                "cli:top_level_entrypoint:impl",
                8,
            ),
            (
                "codex-rs/exec/src/main.rs",
                "exec:binary_entrypoint:impl",
                9,
            ),
            ("codex-rs/exec/src/cli.rs", "exec:cli_options:impl", 10),
            ("codex-rs/exec/src/lib.rs", "exec:runtime:impl", 9),
            ("codex-rs/exec/src/exec_events.rs", "exec:events:impl", 9),
            (
                "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
                "exec:jsonl_event_processor:impl",
                9,
            ),
            (
                "codex-rs/exec/src/event_processor.rs",
                "exec:event_processor:impl",
                8,
            ),
        ];

        for (path, expected_key, expected_score) in expected {
            let hit = search_plan_test_hit(
                path,
                path,
                Path::new(path),
                1,
                SearchHitOrigin::TextMatch,
                false,
            );
            let coverage = architecture_coverage_for_hit(&hit)
                .unwrap_or_else(|| panic!("expected coverage for {path}"));
            assert_eq!(coverage.key, expected_key);
            assert_eq!(coverage.score, expected_score);
        }
    }

    #[test]
    fn architecture_coverage_promotes_payload_content_flow_surfaces() {
        let expected = [
            ("src/payload.config.ts", "payload:config:impl", 9),
            (
                "src/collections/Posts.ts",
                "payload:posts_collection:impl",
                10,
            ),
            (
                "src/collections/Comments.ts",
                "payload:comments_collection:impl",
                10,
            ),
            (
                "src/app/(frontend)/posts/[slug]/comments/route.ts",
                "comments:submission_route:impl",
                10,
            ),
            ("src/app/feed.xml/route.ts", "feed:rss_route:impl", 10),
            ("src/lib/payload.ts", "payload:client:impl", 10),
            (
                "src/lib/content-data/post-content.ts",
                "content:post_data:impl",
                10,
            ),
            (
                "src/lib/content-data/comment-content.ts",
                "content:comment_data:impl",
                10,
            ),
        ];

        for (path, expected_key, expected_score) in expected {
            let hit = search_plan_test_hit(
                path,
                path,
                Path::new(path),
                1,
                SearchHitOrigin::TextMatch,
                false,
            );
            let coverage = architecture_coverage_for_hit(&hit)
                .unwrap_or_else(|| panic!("expected coverage for {path}"));
            assert_eq!(coverage.key, expected_key);
            assert_eq!(coverage.score, expected_score);
        }
    }

    #[test]
    fn architecture_cross_source_coverage_admits_late_payload_content_surfaces() {
        let query = "Explain how Root & Runtime public writing and social surfaces connect through Payload collections, post rendering, comment auth/submission, RSS, and the Elsewhere feed.";
        let mut indexed_hits = Vec::new();
        let indexed_candidates = Vec::new();
        let mut repo_text_hits = (0..10)
            .map(|index| {
                search_plan_test_hit(
                    &format!("generic-payload-{index}"),
                    &format!("src/app/(payload)/admin/importMap{index}.js"),
                    Path::new(&format!("src/app/(payload)/admin/importMap{index}.js")),
                    1,
                    SearchHitOrigin::TextMatch,
                    false,
                )
            })
            .collect::<Vec<_>>();
        let mut repo_text_candidates = repo_text_hits.clone();
        for path in [
            "src/collections/Posts.ts",
            "src/collections/Comments.ts",
            "src/app/(frontend)/posts/[slug]/comments/route.ts",
            "src/app/feed.xml/route.ts",
            "src/lib/payload.ts",
            "src/lib/content-data/post-content.ts",
            "src/lib/content-data/comment-content.ts",
        ] {
            repo_text_candidates.push(search_plan_test_hit(
                path,
                path,
                Path::new(path),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ));
        }

        apply_architecture_cross_source_coverage(
            query,
            &mut indexed_hits,
            &mut repo_text_hits,
            &indexed_candidates,
            &repo_text_candidates,
            10,
        );

        let repo_text_paths = repo_text_hits
            .iter()
            .filter_map(|hit| hit.file_path.as_deref())
            .collect::<Vec<_>>();
        for expected in [
            "src/collections/Posts.ts",
            "src/collections/Comments.ts",
            "src/app/(frontend)/posts/[slug]/comments/route.ts",
            "src/app/feed.xml/route.ts",
            "src/lib/payload.ts",
            "src/lib/content-data/post-content.ts",
            "src/lib/content-data/comment-content.ts",
        ] {
            assert!(
                repo_text_paths.contains(&expected),
                "expected late Payload content surface `{expected}` in {repo_text_paths:#?}"
            );
        }
        assert_eq!(repo_text_paths.len(), 10);
    }

    #[test]
    fn architecture_cross_source_coverage_admits_late_exec_flow_surfaces() {
        let query = "Explain how codex exec --json flows from the top-level CLI into the exec runtime and JSONL event output.";
        let mut indexed_hits = vec![search_plan_test_hit(
            "exec-cli",
            "Cli",
            Path::new("codex-rs/exec/src/cli.rs"),
            14,
            SearchHitOrigin::IndexedSymbol,
            true,
        )];
        for index in 0..9 {
            indexed_hits.push(search_plan_test_hit(
                &format!("generic-cli-{index}"),
                "Cli",
                Path::new(&format!("codex-rs/generic-{index}/src/cli.rs")),
                1,
                SearchHitOrigin::IndexedSymbol,
                true,
            ));
        }
        let indexed_candidates = indexed_hits.clone();

        let mut repo_text_hits = vec![search_plan_test_hit(
            "exec-events",
            "codex-rs/exec/src/exec_events.rs",
            Path::new("codex-rs/exec/src/exec_events.rs"),
            8,
            SearchHitOrigin::TextMatch,
            false,
        )];
        for index in 0..9 {
            repo_text_hits.push(search_plan_test_hit(
                &format!("generic-client-{index}"),
                &format!("codex-rs/generic-{index}/src/client.rs"),
                Path::new(&format!("codex-rs/generic-{index}/src/client.rs")),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ));
        }
        let mut repo_text_candidates = repo_text_hits.clone();
        for path in [
            "codex-rs/cli/src/main.rs",
            "codex-rs/exec/src/main.rs",
            "codex-rs/exec/src/lib.rs",
        ] {
            repo_text_candidates.push(search_plan_test_hit(
                path,
                path,
                Path::new(path),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ));
        }

        apply_architecture_cross_source_coverage(
            query,
            &mut indexed_hits,
            &mut repo_text_hits,
            &indexed_candidates,
            &repo_text_candidates,
            10,
        );

        let repo_text_paths = repo_text_hits
            .iter()
            .filter_map(|hit| hit.file_path.as_deref())
            .collect::<Vec<_>>();
        for expected in [
            "codex-rs/exec/src/exec_events.rs",
            "codex-rs/cli/src/main.rs",
            "codex-rs/exec/src/main.rs",
            "codex-rs/exec/src/lib.rs",
        ] {
            assert!(
                repo_text_paths.contains(&expected),
                "expected exec-flow surface `{expected}` in {repo_text_paths:#?}"
            );
        }
    }

    #[test]
    fn architecture_cross_source_coverage_admits_late_indexed_exec_flow_surfaces() {
        let query = "Explain how codex exec --json flows from the top-level CLI into the exec runtime and JSONL event output.";
        let mut indexed_hits = vec![
            search_plan_test_hit(
                "cli-main",
                "Subcommand::Exec",
                Path::new("codex-rs/cli/src/main.rs"),
                120,
                SearchHitOrigin::IndexedSymbol,
                true,
            ),
            search_plan_test_hit(
                "exec-lib",
                "run_exec_session",
                Path::new("codex-rs/exec/src/lib.rs"),
                1,
                SearchHitOrigin::IndexedSymbol,
                true,
            ),
        ];
        for index in 0..8 {
            indexed_hits.push(search_plan_test_hit(
                &format!("app-server-noise-{index}"),
                "CommandExec",
                Path::new(&format!(
                    "codex-rs/app-server-protocol/src/protocol/v2/noise_{index}.rs"
                )),
                1,
                SearchHitOrigin::IndexedSymbol,
                true,
            ));
        }
        let mut indexed_candidates = indexed_hits.clone();
        for (id, name, path) in [
            ("exec-cli", "Cli", "codex-rs/exec/src/cli.rs"),
            ("exec-main", "clap::Parser", "codex-rs/exec/src/main.rs"),
            (
                "exec-jsonl",
                "EventProcessorWithJsonOutput::emit",
                "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
            ),
            (
                "exec-events",
                "codex_protocol::models::WebSearchAction",
                "codex-rs/exec/src/exec_events.rs",
            ),
        ] {
            indexed_candidates.push(search_plan_test_hit(
                id,
                name,
                Path::new(path),
                1,
                SearchHitOrigin::IndexedSymbol,
                true,
            ));
        }
        let mut repo_text_hits = Vec::new();

        apply_architecture_cross_source_coverage(
            query,
            &mut indexed_hits,
            &mut repo_text_hits,
            &indexed_candidates,
            &[],
            10,
        );

        let indexed_paths = indexed_hits
            .iter()
            .filter_map(|hit| hit.file_path.as_deref())
            .collect::<Vec<_>>();
        for expected in [
            "codex-rs/exec/src/cli.rs",
            "codex-rs/exec/src/main.rs",
            "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
            "codex-rs/exec/src/exec_events.rs",
        ] {
            assert!(
                indexed_paths.contains(&expected),
                "expected late indexed exec-flow surface `{expected}` in {indexed_paths:#?}"
            );
        }
        assert_eq!(indexed_paths.len(), 10);
    }

    #[test]
    fn repo_text_window_does_not_diversify_non_architecture_queries() {
        let mut hits = vec![
            search_plan_test_hit(
                "first",
                "first",
                Path::new("crates/codestory-runtime/src/lib.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "second",
                "second",
                Path::new("crates/codestory-indexer/src/lib.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
            search_plan_test_hit(
                "third",
                "third",
                Path::new("crates/codestory-store/src/storage_impl/mod.rs"),
                1,
                SearchHitOrigin::TextMatch,
                false,
            ),
        ];

        truncate_repo_text_hits_for_query("run_index", &mut hits, 2);

        assert_eq!(
            hits.iter()
                .map(|hit| hit.node_id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
    }

    #[test]
    fn search_plan_anchor_groups_keep_diverse_names_before_truncation() {
        let temp = tempdir().expect("create temp dir");
        let source_path = temp.path().join("src").join("flow.rs");
        fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
        fs::write(&source_path, "fn placeholder() {}\n").expect("write source");
        let mut hits = (0..10)
            .map(|index| {
                search_plan_test_hit(
                    &format!("cli-{index}"),
                    "cli",
                    &source_path,
                    index + 1,
                    SearchHitOrigin::IndexedSymbol,
                    true,
                )
            })
            .collect::<Vec<_>>();
        hits.push(search_plan_test_hit(
            "workspace",
            "WorkspaceManifest::build_execution_plan",
            &source_path,
            20,
            SearchHitOrigin::IndexedSymbol,
            true,
        ));
        hits.push(search_plan_test_hit(
            "indexer",
            "WorkspaceIndexer::run",
            &source_path,
            21,
            SearchHitOrigin::IndexedSymbol,
            true,
        ));

        let terms = search_plan_terms(
            "Explain how the CLI runtime workspace indexer store and search flow fits together.",
        );
        let groups = search_plan_anchor_groups(
            "Explain how the CLI runtime workspace indexer store and search flow fits together.",
            &terms,
            &hits,
            &[],
            &[],
            &HashMap::new(),
        );
        let anchors = groups
            .iter()
            .map(|group| group.anchor.as_str())
            .collect::<Vec<_>>();
        assert!(
            anchors
                .iter()
                .any(|anchor| anchor.contains("WorkspaceManifest")),
            "duplicate cli anchors should not crowd out workspace anchor: {anchors:#?}"
        );
        assert!(
            anchors
                .iter()
                .any(|anchor| anchor.contains("WorkspaceIndexer")),
            "duplicate cli anchors should not crowd out indexer anchor: {anchors:#?}"
        );
    }

    #[test]
    fn search_plan_ranks_active_callers_above_definition_only_anchors() {
        let temp = tempdir().expect("create temp dir");
        let source_path = temp.path().join("src").join("feed.rs");
        fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
        fs::write(
            &source_path,
            "pub fn getLatestSocialEntries() {}\npub fn getElsewhereFeed() {}\n",
        )
        .expect("write source");
        let active = search_plan_test_hit(
            "active",
            "getLatestSocialEntries",
            &source_path,
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        );
        let definition_only = search_plan_test_hit(
            "definition",
            "getElsewhereFeed",
            &source_path,
            2,
            SearchHitOrigin::IndexedSymbol,
            true,
        );
        let query = "getElsewhereFeed latest social feed";
        let terms = search_plan_terms(query);
        let active_path_evidence = HashMap::from([
            (
                active.node_id.clone(),
                SearchPlanActivePathEvidence { caller_count: 2 },
            ),
            (
                definition_only.node_id.clone(),
                SearchPlanActivePathEvidence { caller_count: 0 },
            ),
        ]);

        let groups = search_plan_anchor_groups(
            query,
            &terms,
            &[definition_only, active],
            &[],
            &[],
            &active_path_evidence,
        );

        assert_eq!(
            groups
                .first()
                .and_then(|group| group.chosen_symbol.as_ref())
                .map(|hit| hit.display_name.as_str()),
            Some("getLatestSocialEntries"),
            "visible production callers should outrank a definition-only exact-name anchor: {groups:#?}"
        );
        assert!(
            groups.iter().any(|group| {
                group.anchor == "getElsewhereFeed"
                    && group.caller_count == 0
                    && group.definition_only
                    && group.no_visible_callers
                    && group
                        .reasons
                        .iter()
                        .any(|reason| reason.contains("no visible production callers"))
            }),
            "definition-only callable anchors should be labeled: {groups:#?}"
        );
    }

    #[test]
    fn search_plan_test_file_names_are_not_visible_production_callers() {
        for path in [
            "src/api.test.ts",
            "src/api.spec.ts",
            "src/api.test.tsx",
            "src/api.spec.jsx",
            "src/__tests__/api.ts",
        ] {
            assert!(
                search_plan_path_is_test_or_bench(path),
                "{path} should be treated as test code for active-path evidence"
            );
        }
    }

    #[test]
    fn search_plan_repo_text_owner_identifier_does_not_promote_member_symbol() {
        let temp = tempdir().expect("create temp dir");
        let source_path = temp.path().join("src").join("lib.rs");
        fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
        fs::write(
            &source_path,
            "pub struct WorkspaceIndexer;\n\nimpl WorkspaceIndexer {\n    pub fn normalize_index_path(&self) {}\n}\n\n\n\n// WorkspaceIndexer coordinates indexing flow\n",
        )
        .expect("write source");
        let member_hit = search_plan_test_hit(
            "member",
            "WorkspaceIndexer::normalize_index_path",
            &source_path,
            4,
            SearchHitOrigin::IndexedSymbol,
            false,
        );
        let repo_hit = search_plan_test_hit(
            "repo",
            "src/lib.rs:9",
            &source_path,
            9,
            SearchHitOrigin::TextMatch,
            false,
        );
        let query = "WorkspaceIndexer indexing flow";
        let terms = search_plan_terms(query);

        let groups = search_plan_anchor_groups(
            query,
            &terms,
            &[],
            &[repo_hit],
            &[member_hit],
            &HashMap::new(),
        );

        assert!(
            groups.iter().any(|group| {
                group.chosen_symbol.is_none()
                    && matches!(
                        group.promotion_status,
                        SearchPlanPromotionStatusDto::Ambiguous
                    )
            }),
            "owner-only repo-text mention should stay unbound instead of promoting to a member: {groups:#?}"
        );
    }

    #[test]
    fn search_plan_repo_text_exact_terminal_identifier_promotes_member_symbol() {
        let temp = tempdir().expect("create temp dir");
        let source_path = temp.path().join("src").join("lib.rs");
        fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
        fs::write(
            &source_path,
            "pub struct WorkspaceIndexer;\n\nimpl WorkspaceIndexer {\n    pub fn normalize_index_path(&self) {}\n}\n\n\n\n// normalize_index_path normalizes storage keys before indexing\n",
        )
        .expect("write source");
        let member_hit = search_plan_test_hit(
            "member",
            "WorkspaceIndexer::normalize_index_path",
            &source_path,
            4,
            SearchHitOrigin::IndexedSymbol,
            false,
        );
        let repo_hit = search_plan_test_hit(
            "repo",
            "src/lib.rs:9",
            &source_path,
            9,
            SearchHitOrigin::TextMatch,
            false,
        );
        let query = "normalize_index_path storage keys";
        let terms = search_plan_terms(query);

        let groups = search_plan_anchor_groups(
            query,
            &terms,
            &[],
            &[repo_hit],
            &[member_hit],
            &HashMap::new(),
        );

        assert!(
            groups.iter().any(|group| {
                group
                    .chosen_symbol
                    .as_ref()
                    .is_some_and(|hit| hit.display_name == "WorkspaceIndexer::normalize_index_path")
                    && group.promotion_method.as_deref() == Some("same_file_exact_identifier")
            }),
            "exact terminal identifier should still promote to the matching member: {groups:#?}"
        );
        let next_actions = search_plan_next_actions(&groups);
        assert!(next_actions.iter().any(|action| {
            action.action == "snippet"
                && action.node_id.0 == "member"
                && action
                    .options
                    .iter()
                    .any(|option| option == "function_body")
        }));
    }

    #[test]
    fn search_plan_speculation_policy_matches_hidden_trail_edges() {
        assert!(search_plan_runtime_call_is_speculative(
            Some(codestory_contracts::graph::ResolutionCertainty::Probable),
            Some(0.70)
        ));
        assert!(search_plan_runtime_call_is_speculative(None, Some(0.84)));
        assert!(!search_plan_runtime_call_is_speculative(
            Some(codestory_contracts::graph::ResolutionCertainty::Certain),
            Some(codestory_contracts::graph::ResolutionCertainty::CERTAIN_MIN)
        ));
    }

    #[test]
    fn repo_explanation_overview_replacement_is_generic_only() {
        assert!(AppController::is_repo_explanation_search_query(
            "Explain how this repo fits together"
        ));
        assert!(!query_has_symbol_or_literal_signal(
            "Explain how this repo fits together"
        ));
        assert!(query_has_symbol_or_literal_signal(
            "Explain how AppController fits into this repo"
        ));
        assert!(query_has_symbol_or_literal_signal(
            "Explain `CODESTORY_EMBED_RUNTIME_MODE` in this repo"
        ));
        assert!(query_has_symbol_or_literal_signal(
            "Explain crates/codestory-runtime/src/lib.rs in this repo"
        ));
    }

    #[test]
    fn file_text_matching_prefers_high_signal_identifier_literals() {
        let contents = r#"
pub const CODESTORY_EMBED_RUNTIME_MODE: &str = "hash";

fn build_llm_symbol_doc_text() -> String {
    String::new()
}
"#;

        assert_eq!(
            file_text_match_line(
                contents,
                "Where is `build_llm_symbol_doc_text` defined?",
                &extract_symbol_search_terms("Where is `build_llm_symbol_doc_text` defined?")
            ),
            Some(4)
        );
        assert_eq!(
            file_text_match_line(
                contents,
                "What sets CODESTORY_EMBED_RUNTIME_MODE?",
                &extract_symbol_search_terms("What sets CODESTORY_EMBED_RUNTIME_MODE?")
            ),
            Some(2)
        );
    }

    #[test]
    fn should_expand_symbol_query_for_sentence_prompts() {
        assert!(should_expand_symbol_query(
            "How does the language parsing work in this repo?",
            0
        ));
        assert!(!should_expand_symbol_query("parser", 0));
        assert!(!should_expand_symbol_query(
            "how does the language parsing work in this repo",
            5
        ));
        assert!(!should_expand_symbol_query(
            "How does the language parsing work in this repo?",
            5
        ));
    }

    #[test]
    fn aggregate_symbol_matches_prioritizes_direct_matches() {
        let direct = vec![(CoreNodeId(7), 2.0)];
        let expanded = vec![(CoreNodeId(7), 99.0), (CoreNodeId(8), 95.0)];
        let merged = crate::support::aggregate_symbol_matches(direct, expanded);
        assert_eq!(merged.first().map(|(id, _)| *id), Some(CoreNodeId(7)));
    }

    #[test]
    fn build_search_hit_prefers_declaration_coordinates_and_filters_unknown_nodes() {
        let mut storage = Storage::new_in_memory().expect("storage");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(10),
                    kind: NodeKind::FILE,
                    serialized_name: "src/lib.rs".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(11),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "check_winner".to_string(),
                    file_node_id: Some(CoreNodeId(10)),
                    start_line: Some(42),
                    start_col: Some(5),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(12),
                    kind: NodeKind::UNKNOWN,
                    serialized_name: "check_winner".to_string(),
                    file_node_id: Some(CoreNodeId(10)),
                    start_line: Some(99),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_occurrences_batch(&[Occurrence {
                element_id: 11,
                kind: OccurrenceKind::REFERENCE,
                location: SourceLocation {
                    file_node_id: CoreNodeId(10),
                    start_line: 87,
                    start_col: 9,
                    end_line: 87,
                    end_col: 20,
                },
            }])
            .expect("insert occurrences");

        let node_names = HashMap::from([
            (CoreNodeId(11), "check_winner".to_string()),
            (CoreNodeId(12), "check_winner".to_string()),
        ]);

        let definition_hit =
            AppController::build_search_hit(&storage, &node_names, CoreNodeId(11), 1.0)
                .expect("definition hit");
        assert_eq!(definition_hit.file_path.as_deref(), Some("src/lib.rs"));
        assert_eq!(definition_hit.line, Some(42));

        assert!(
            AppController::build_search_hit(&storage, &node_names, CoreNodeId(12), 1.0).is_none(),
            "unknown placeholder nodes should be dropped from indexed results"
        );
    }

    #[test]
    fn build_search_hit_adjusts_route_scores_by_extraction_provenance() {
        fn route_canonical_id(extraction: &str) -> String {
            format!(
                "route_endpoint:{}",
                serde_json::json!({
                    "kind": "framework_route",
                    "framework": "express",
                    "method": "GET",
                    "path": "/api/users",
                    "provenance": [format!("extraction:{extraction}")],
                })
            )
        }

        let mut storage = Storage::new_in_memory().expect("storage");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(20),
                    kind: NodeKind::FILE,
                    serialized_name: "src/routes.ts".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(22),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "GET /api/users".to_string(),
                    file_node_id: Some(CoreNodeId(20)),
                    canonical_id: Some(route_canonical_id("ast_indexed")),
                    start_line: Some(3),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(23),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "GET /api/users".to_string(),
                    file_node_id: Some(CoreNodeId(20)),
                    canonical_id: Some(route_canonical_id("text_only")),
                    start_line: Some(3),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(24),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "plain_handler".to_string(),
                    file_node_id: Some(CoreNodeId(20)),
                    start_line: Some(8),
                    ..Default::default()
                },
            ])
            .expect("insert route nodes");
        let node_names = HashMap::from([
            (CoreNodeId(22), "GET /api/users".to_string()),
            (CoreNodeId(23), "GET /api/users".to_string()),
            (CoreNodeId(24), "plain_handler".to_string()),
        ]);

        let ast = AppController::build_search_hit(&storage, &node_names, CoreNodeId(22), 1.0)
            .expect("ast route hit");
        let text_only = AppController::build_search_hit(&storage, &node_names, CoreNodeId(23), 1.0)
            .expect("text-only route hit");
        let normal = AppController::build_search_hit(&storage, &node_names, CoreNodeId(24), 1.0)
            .expect("normal hit");

        assert!(
            ast.score > text_only.score,
            "AST-indexed route evidence should outrank otherwise equivalent text-only route guesses"
        );
        assert!(ast.score > normal.score);
        assert!(text_only.score < normal.score);
        assert_eq!(normal.score, 1.0);

        let mut hits = [text_only, ast.clone()];
        hits.sort_by(|left, right| compare_search_hits("/api/users", left, right));
        assert_eq!(hits.first().map(|hit| &hit.node_id), Some(&ast.node_id));
    }

    #[test]
    fn build_search_state_scopes_projection_rebuild_to_touched_files() {
        let mut storage = Storage::new_in_memory().expect("storage");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(900),
                    kind: NodeKind::FILE,
                    serialized_name: "src/changed.rs".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(901),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "old_name".to_string(),
                    qualified_name: Some("pkg::old_name".to_string()),
                    file_node_id: Some(CoreNodeId(900)),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(910),
                    kind: NodeKind::FILE,
                    serialized_name: "src/untouched.rs".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(911),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "untouched".to_string(),
                    qualified_name: Some("pkg::untouched".to_string()),
                    file_node_id: Some(CoreNodeId(910)),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .rebuild_search_symbol_projection_from_node_table()
            .expect("full projection");

        storage
            .insert_nodes_batch(&[Node {
                id: CoreNodeId(901),
                kind: NodeKind::FUNCTION,
                serialized_name: "renamed".to_string(),
                qualified_name: Some("pkg::renamed".to_string()),
                file_node_id: Some(CoreNodeId(900)),
                ..Default::default()
            }])
            .expect("update changed node");
        storage
            .upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
                node_id: CoreNodeId(911),
                display_name: "stale_other_file".to_string(),
            }])
            .expect("seed untouched stale projection");

        let nodes = storage.get_nodes().expect("nodes");
        let touched = HashSet::from([CoreNodeId(900)]);
        build_search_state(
            &mut storage,
            None,
            nodes,
            Some(&touched),
            SemanticProjectionMode::SkipPersistence,
            false,
        )
        .expect("scoped search state");

        let projection = storage
            .get_search_symbol_projection_batch_after(None, 10)
            .expect("projection");
        let names_by_id: HashMap<_, _> = projection
            .into_iter()
            .map(|entry| (entry.node_id, entry.display_name))
            .collect();
        assert_eq!(
            names_by_id.get(&CoreNodeId(900)).map(String::as_str),
            Some("src/changed.rs")
        );
        assert_eq!(
            names_by_id.get(&CoreNodeId(901)).map(String::as_str),
            Some("pkg::renamed")
        );
        assert_eq!(
            names_by_id.get(&CoreNodeId(911)).map(String::as_str),
            Some("stale_other_file")
        );
    }

    #[test]
    fn search_requires_full_sidecars_for_exact_type_queries() {
        let temp = tempdir().expect("create temp dir");
        let db_path = temp.path().join("codestory.db");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(10),
                        kind: NodeKind::FILE,
                        serialized_name: temp
                            .path()
                            .join("src")
                            .join("lib.rs")
                            .to_string_lossy()
                            .to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(11),
                        kind: NodeKind::STRUCT,
                        serialized_name: "AppController".to_string(),
                        file_node_id: Some(CoreNodeId(10)),
                        start_line: Some(10),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(12),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "AppController::open_project".to_string(),
                        qualified_name: Some("AppController::open_project".to_string()),
                        file_node_id: Some(CoreNodeId(10)),
                        start_line: Some(20),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(13),
                        kind: NodeKind::UNKNOWN,
                        serialized_name: "AppController".to_string(),
                        file_node_id: Some(CoreNodeId(10)),
                        start_line: Some(30),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");

        let error = controller
            .search(SearchRequest {
                query: "AppController".to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 10,
                expand_search_plan: false,
                hybrid_weights: None,
                hybrid_limits: None,
            })
            .expect_err("search should require full sidecars");
        assert_mandatory_sidecar_unavailable(&error);
    }

    #[test]
    fn compare_search_hits_prefers_function_over_method_for_equal_symbol_matches() {
        let function = SearchHit {
            node_id: NodeId("function".to_string()),
            display_name: "ArtificialPlayer::min_max".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: None,
            line: None,
            score: 184.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        };
        let method = SearchHit {
            node_id: NodeId("method".to_string()),
            display_name: "ArtificialPlayer::min_max".to_string(),
            kind: codestory_contracts::api::NodeKind::METHOD,
            file_path: None,
            line: None,
            score: 184.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        };

        let mut hits = [method, function.clone()];
        hits.sort_by(|left, right| compare_search_hits("min_max", left, right));

        assert_eq!(hits.first().map(|hit| hit.kind), Some(function.kind));
    }

    #[test]
    fn search_prefers_full_sidecars_for_tictactoe_queries() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::set(HYBRID_RETRIEVAL_ENABLED_ENV, "false");
        let workspace = copy_tictactoe_workspace();
        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: workspace.path().to_string_lossy().to_string(),
            })
            .expect("open workspace");
        controller
            .run_indexing_blocking(IndexMode::Full)
            .expect("index fixtures");

        for query in ["check_winner", "min_max"] {
            let error = controller
                .search(SearchRequest {
                    query: query.to_string(),
                    repo_text: SearchRepoTextMode::Off,
                    limit_per_source: 10,
                    expand_search_plan: false,
                    hybrid_weights: None,
                    hybrid_limits: None,
                })
                .expect_err("search fixtures should require full sidecars");
            assert_mandatory_sidecar_unavailable(&error);
        }
    }

    #[test]
    fn repo_explanation_search_requires_full_sidecar_retrieval() {
        let _lock = ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env = EnvGuard::set(HYBRID_RETRIEVAL_ENABLED_ENV, "false");
        let workspace = copy_tictactoe_workspace();
        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: workspace.path().to_string_lossy().to_string(),
            })
            .expect("open workspace");
        controller
            .run_indexing_blocking(IndexMode::Full)
            .expect("index fixtures");

        let generic_error = controller
            .search_results(SearchRequest {
                query: "Explain how this repo fits together".to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 10,
                expand_search_plan: false,
                hybrid_weights: None,
                hybrid_limits: None,
            })
            .expect_err("generic repo explanation search should require full sidecars");
        assert_mandatory_sidecar_unavailable(&generic_error);

        let symbol_error = controller
            .search_results(SearchRequest {
                query: "Explain how check_winner fits in this repo".to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 10,
                expand_search_plan: true,
                hybrid_weights: None,
                hybrid_limits: None,
            })
            .expect_err("symbol-like repo explanation search should require full sidecars");
        assert_mandatory_sidecar_unavailable(&symbol_error);
    }

    #[test]
    fn search_rejects_natural_language_queries_without_full_sidecars() {
        let temp = tempdir().expect("create temp dir");
        let db_path = temp.path().join("codestory.db");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(201),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "language_parsing_pipeline".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(202),
                        kind: NodeKind::MODULE,
                        serialized_name: "parser_core".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(203),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "runtime_workspace_indexer_store_flow".to_string(),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
        }

        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let broad_query =
            "Explain how the full-index path flows through runtime workspace indexer and store";
        let error_without_plan = controller
            .search_results(SearchRequest {
                query: broad_query.to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 20,
                expand_search_plan: false,
                hybrid_weights: None,
                hybrid_limits: None,
            })
            .expect_err("natural language search should require full sidecars");
        assert_mandatory_sidecar_unavailable(&error_without_plan);

        let error_with_plan = controller
            .search_results(SearchRequest {
                query: broad_query.to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 20,
                expand_search_plan: true,
                hybrid_weights: None,
                hybrid_limits: None,
            })
            .expect_err("natural language search plan should require full sidecars");
        assert_mandatory_sidecar_unavailable(&error_with_plan);
    }

    #[test]
    fn build_search_state_prefers_qualified_name() {
        let mut storage = Storage::new_in_memory().expect("storage");
        let nodes = vec![Node {
            id: CoreNodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "short_name".to_string(),
            qualified_name: Some("pkg.mod.short_name".to_string()),
            ..Default::default()
        }];

        let result = build_search_state(
            &mut storage,
            None,
            nodes,
            None,
            SemanticProjectionMode::SkipPersistence,
            true,
        )
        .expect("build search state");
        let node_names = result.node_names;
        let engine = result.engine;
        assert_eq!(
            node_names.get(&CoreNodeId(1)).map(String::as_str),
            Some("pkg.mod.short_name")
        );

        let hits = engine.search_symbol("pkg.mod");
        assert_eq!(hits.first().copied(), Some(CoreNodeId(1)));
    }

    #[test]
    fn open_project_summary_clears_search_state() {
        let temp = tempdir().expect("create temp dir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), storage_path.clone())
            .expect("open project with search state");
        assert!(
            controller.state.lock().search_engine.is_some(),
            "expected full open to initialize search state"
        );

        controller
            .open_project_summary_with_storage_path(temp.path().to_path_buf(), storage_path)
            .expect("open project summary");
        let state = controller.state.lock();
        assert!(state.search_engine.is_none());
        assert!(state.node_names.is_empty());
    }

    #[test]
    fn run_indexing_without_runtime_refresh_keeps_search_uninitialized() {
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(workspace.path().to_path_buf(), storage_path)
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index without runtime refresh");

        let state = controller.state.lock();
        assert!(!state.is_indexing);
        assert!(state.search_engine.is_none());
        assert!(state.node_names.is_empty());
    }

    #[test]
    fn run_indexing_without_runtime_refresh_populates_semantic_docs_in_storage() {
        let _env = hybrid_test_env();
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index without runtime refresh");

        let state = controller.state.lock();
        assert!(!state.is_indexing);
        assert!(state.search_engine.is_none());
        assert!(state.node_names.is_empty());
        drop(state);

        let storage = Storage::open(&storage_path).expect("reopen storage");
        let stats = storage
            .get_llm_symbol_doc_stats()
            .expect("semantic doc stats");
        assert!(
            stats.doc_count > 0,
            "expected full indexing to persist semantic docs without requiring a follow-up open"
        );
    }

    #[test]
    fn full_refresh_reuses_unchanged_semantic_docs_from_previous_live_index() {
        let _env = hybrid_test_env();
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");
        let first_timings = controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("first full index");
        assert!(
            first_timings.semantic_docs_embedded.unwrap_or(0) > 0,
            "initial full refresh should embed semantic docs"
        );
        assert_eq!(first_timings.semantic_docs_reused.unwrap_or(0), 0);

        let first_docs = Storage::open(&storage_path)
            .expect("open first storage")
            .get_all_llm_symbol_docs()
            .expect("first semantic docs");
        assert!(
            first_docs
                .iter()
                .all(|doc| doc.doc_version == LLM_SYMBOL_DOC_SCHEMA_VERSION
                    && !doc.doc_hash.is_empty()),
            "semantic docs should carry reuse metadata"
        );

        let second_timings = controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("second full index");
        assert!(
            second_timings.cache_refresh_ms.unwrap_or(0) > 0,
            "cache refresh timing should include persisted search plus semantic sync"
        );
        assert!(
            second_timings.semantic_doc_build_ms.is_some(),
            "semantic doc build timing should be reported separately"
        );
        assert_eq!(
            second_timings.semantic_docs_embedded.unwrap_or(u32::MAX),
            0,
            "unchanged full refresh should not re-embed semantic docs"
        );
        assert!(
            second_timings.semantic_docs_reused.unwrap_or(0) > 0,
            "unchanged full refresh should reuse semantic docs copied into the staged DB"
        );
    }

    #[test]
    fn full_refresh_repairs_reused_semantic_docs_missing_contract_metadata() {
        let _env = hybrid_test_env();
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("first full index");

        let mut legacy_docs = Storage::open(&storage_path)
            .expect("open storage before legacy rewrite")
            .get_all_llm_symbol_docs()
            .expect("semantic docs before legacy rewrite");
        assert!(
            !legacy_docs.is_empty(),
            "initial full index should persist semantic docs"
        );
        for doc in &mut legacy_docs {
            doc.embedding_profile = None;
            doc.embedding_backend = None;
            doc.doc_shape = None;
        }
        Storage::open(&storage_path)
            .expect("reopen storage for legacy rewrite")
            .upsert_llm_symbol_docs_batch(&legacy_docs)
            .expect("rewrite legacy semantic docs");

        let repair_timings = controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("full refresh repairs legacy contract metadata");
        assert!(
            repair_timings.semantic_docs_embedded.unwrap_or(0) > 0,
            "missing contract metadata should prevent stale semantic docs from being reused"
        );

        let repaired_docs = Storage::open(&storage_path)
            .expect("open storage after repair")
            .get_all_llm_symbol_docs()
            .expect("semantic docs after repair");
        assert!(
            repaired_docs.iter().all(|doc| {
                doc.embedding_profile.as_deref() == Some("bge-small-en-v1.5")
                    && doc.embedding_backend.as_deref() == Some("hash")
                    && doc.doc_shape.as_deref() == Some(semantic_doc_shape_contract().as_str())
            }),
            "full refresh should backfill reusable docs with the current semantic contract"
        );
    }

    #[test]
    fn full_refresh_rebuilds_semantic_docs_when_embedding_dimension_changes() {
        let mut env = hybrid_test_env();
        env.push(EnvGuard::set(EMBEDDING_EXPECTED_DIM_ENV, "128"));
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("first full index");

        let first_docs = Storage::open(&storage_path)
            .expect("open storage after first index")
            .get_all_llm_symbol_docs()
            .expect("semantic docs after first index");
        assert!(
            first_docs
                .iter()
                .all(|doc| doc.embedding_dim == 128 && doc.embedding.len() == 128),
            "initial hash docs should use the configured dimension"
        );

        env.push(EnvGuard::set(EMBEDDING_EXPECTED_DIM_ENV, "384"));
        let repair_timings = controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("second full index after dimension change");
        assert!(
            repair_timings.semantic_docs_embedded.unwrap_or(0) > 0,
            "dimension drift should rebuild semantic docs instead of reusing stale vectors"
        );

        let repaired_docs = Storage::open(&storage_path)
            .expect("open storage after dimension repair")
            .get_all_llm_symbol_docs()
            .expect("semantic docs after dimension repair");
        assert!(
            repaired_docs
                .iter()
                .all(|doc| doc.embedding_dim == 384 && doc.embedding.len() == 384),
            "full refresh should persist semantic docs with the new dimension"
        );
    }

    #[test]
    fn incremental_refresh_rebuilds_touched_file_semantic_docs_only() {
        let _env = hybrid_test_env();
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("full index");
        let before_docs = Storage::open(&storage_path)
            .expect("reopen storage before incremental")
            .get_all_llm_symbol_docs()
            .expect("semantic docs before incremental");

        let rust_fixture = workspace.path().join("rust_tictactoe.rs");
        let mut source = fs::read_to_string(&rust_fixture).expect("read rust fixture");
        source.push_str("\nfn codestory_added_move_hint() -> i32 { 42 }\n");
        fs::write(&rust_fixture, source).expect("write changed rust fixture");

        let incremental_timings = controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
            .expect("incremental index");
        assert!(
            incremental_timings.symbol_search_docs_written.unwrap_or(0) > 0,
            "new symbols from the touched file should update graph-native symbol docs"
        );
        if incremental_timings.semantic_docs_embedded.unwrap_or(0) > 0 {
            assert!(
                incremental_timings
                    .semantic_docs_embedded
                    .unwrap_or(u32::MAX)
                    < clamp_usize_to_u32(before_docs.len()),
                "incremental dense sync should not re-embed untouched files"
            );
        }
        assert_eq!(
            incremental_timings.semantic_docs_stale.unwrap_or(0),
            0,
            "adding a symbol should not make existing semantic docs stale"
        );

        let docs = Storage::open(&storage_path)
            .expect("reopen storage")
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("symbol docs after incremental");
        assert!(
            docs.iter()
                .any(|doc| doc.display_name.contains("codestory_added_move_hint")),
            "incremental symbol docs should include the new symbol"
        );
    }

    #[test]
    fn incremental_refresh_rebuilds_all_semantic_docs_when_embedding_contract_changes() {
        let mut env = hybrid_test_env();
        env.push(EnvGuard::set(EMBEDDING_EXPECTED_DIM_ENV, "128"));
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("initial full index");

        let before_docs = Storage::open(&storage_path)
            .expect("open storage before drift")
            .get_all_llm_symbol_docs()
            .expect("semantic docs before drift");
        assert!(
            !before_docs.is_empty(),
            "fixture should persist semantic docs"
        );
        assert!(
            before_docs
                .iter()
                .all(|doc| doc.embedding_dim == 128 && doc.embedding.len() == 128),
            "initial docs should use the first embedding contract"
        );

        env.push(EnvGuard::set(EMBEDDING_EXPECTED_DIM_ENV, "384"));
        let rust_fixture = workspace.path().join("rust_tictactoe.rs");
        let mut source = fs::read_to_string(&rust_fixture).expect("read rust fixture");
        source.push_str("\nfn codestory_contract_drift_added_hint() -> i32 { 7 }\n");
        fs::write(&rust_fixture, source).expect("write changed rust fixture");

        let incremental_timings = controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
            .expect("incremental index after contract drift");
        assert!(
            incremental_timings.semantic_docs_embedded.unwrap_or(0)
                >= clamp_usize_to_u32(before_docs.len()),
            "contract drift should expand incremental semantic sync beyond the touched file"
        );

        let repaired_docs = Storage::open(&storage_path)
            .expect("open storage after drift repair")
            .get_all_llm_symbol_docs()
            .expect("semantic docs after drift repair");
        assert!(
            repaired_docs
                .iter()
                .all(|doc| doc.embedding_dim == 384 && doc.embedding.len() == 384),
            "incremental repair should leave all stored semantic docs on the current contract"
        );
        let repaired_symbol_docs = Storage::open(&storage_path)
            .expect("open storage after drift repair for symbol docs")
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("symbol docs after drift repair");
        assert!(
            repaired_symbol_docs.iter().any(|doc| doc
                .display_name
                .contains("codestory_contract_drift_added_hint")),
            "incremental repair should still include symbol docs from the touched file"
        );
    }

    #[test]
    fn grounding_snapshot_from_summary_open_keeps_search_state_cold() {
        let _env = hybrid_test_env();
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index without runtime refresh");

        {
            let state = controller.state.lock();
            assert!(
                state.search_engine.is_none(),
                "summary open plus indexing should leave search state unloaded"
            );
            assert!(
                state.node_names.is_empty(),
                "summary open plus indexing should leave node label cache empty"
            );
        }

        let snapshot = controller
            .grounding_snapshot(GroundingBudgetDto::Balanced)
            .expect("grounding snapshot");
        assert_eq!(
            snapshot.retrieval.as_ref().map(|state| state.mode),
            Some(RetrievalModeDto::Hybrid)
        );

        let state = controller.state.lock();
        assert!(
            state.search_engine.is_none(),
            "grounding snapshot should not rebuild the full search engine"
        );
        assert!(
            state.node_names.is_empty(),
            "grounding snapshot should not repopulate node labels from search state"
        );
    }

    #[test]
    fn retrieval_state_from_summary_open_keeps_search_state_cold() {
        let _env = hybrid_test_env();
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(workspace.path().to_path_buf(), storage_path)
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index without runtime refresh");

        let retrieval = controller.retrieval_state().expect("retrieval state");
        assert_eq!(retrieval.mode, RetrievalModeDto::Hybrid);

        let state = controller.state.lock();
        assert!(
            state.search_engine.is_none(),
            "retrieval_state should stay storage-backed on a cold controller"
        );
        assert!(
            state.node_names.is_empty(),
            "retrieval_state should not populate search labels on a cold controller"
        );
    }

    #[test]
    fn search_results_ignores_repo_text_hits_without_full_sidecars() {
        let temp = tempdir().expect("temp dir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
        let source_path = temp.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
        std::fs::write(
            &source_path,
            "fn alpha() {}\n// this explains how alpha work items flow through the runtime\n",
        )
        .expect("write source");

        {
            let mut storage = Storage::open(&storage_path).expect("open storage");
            storage
                .insert_file(&FileInfo {
                    id: 11,
                    path: source_path.clone(),
                    language: "rust".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 2,
                    file_role: codestory_store::FileRole::Source,
                })
                .expect("insert file");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(11),
                        kind: NodeKind::FILE,
                        serialized_name: source_path.to_string_lossy().to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(101),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "alpha".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        start_line: Some(1),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), storage_path)
            .expect("open project");

        let error = controller
            .search_results(SearchRequest {
                query: "how does alpha work".to_string(),
                repo_text: SearchRepoTextMode::On,
                limit_per_source: 5,
                expand_search_plan: false,
                hybrid_weights: None,
                hybrid_limits: None,
            })
            .expect_err("repo-text search should still require full sidecars");
        assert_mandatory_sidecar_unavailable(&error);
    }

    #[test]
    fn repo_text_auto_fallback_is_not_product_search_without_full_sidecars() {
        let temp = tempdir().expect("temp dir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
        let source_path = temp.path().join("src").join("lib.rs");
        let readme_path = temp.path().join("README.md");
        std::fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
        std::fs::write(&source_path, "pub fn unrelated_anchor() {}\n").expect("write source");
        std::fs::write(
            &readme_path,
            "GlobalResourceListView is a retired frontend surface mentioned in notes.\n",
        )
        .expect("write readme");

        {
            let mut storage = Storage::open(&storage_path).expect("open storage");
            storage
                .insert_file(&FileInfo {
                    id: 11,
                    path: source_path.clone(),
                    language: "rust".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: codestory_store::FileRole::Source,
                })
                .expect("insert source file");
            storage
                .insert_file(&FileInfo {
                    id: 12,
                    path: readme_path,
                    language: "markdown".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: codestory_store::FileRole::Source,
                })
                .expect("insert readme file");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(11),
                        kind: NodeKind::FILE,
                        serialized_name: source_path.to_string_lossy().to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(101),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "unrelated_anchor".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        start_line: Some(1),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), storage_path)
            .expect("open project");

        let error = controller
            .search_results(SearchRequest {
                query: "GlobalResourceListView".to_string(),
                repo_text: SearchRepoTextMode::Auto,
                limit_per_source: 5,
                expand_search_plan: false,
                hybrid_weights: None,
                hybrid_limits: None,
            })
            .expect_err("repo-text auto fallback should require full sidecars");
        assert_mandatory_sidecar_unavailable(&error);
    }

    #[test]
    fn repo_text_ranking_uses_path_and_query_tokens_for_svelte_surfaces() {
        let temp = tempdir().expect("temp dir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
        let rust_path = temp.path().join("src").join("commands.rs");
        let svelte_path = temp.path().join("src").join("App.svelte");
        std::fs::create_dir_all(rust_path.parent().expect("src parent")).expect("create src");
        std::fs::write(
            &rust_path,
            "pub fn get_snapshot() {}\n// invoke runtime bridge\n",
        )
        .expect("write rust");
        std::fs::write(
            &svelte_path,
            "const readSnapshot = () => invoke('get_snapshot');\n",
        )
        .expect("write svelte");

        {
            let storage = Storage::open(&storage_path).expect("open storage");
            for (id, path, language) in
                [(11, rust_path, "rust"), (12, svelte_path.clone(), "svelte")]
            {
                storage
                    .insert_file(&FileInfo {
                        id,
                        path,
                        language: language.to_string(),
                        modification_time: 1,
                        indexed: true,
                        complete: true,
                        line_count: 1,
                        file_role: codestory_store::FileRole::Source,
                    })
                    .expect("insert file");
            }
        }

        let storage = Storage::open(&storage_path).expect("reopen storage");
        let scan = AppController::collect_repo_text_hits(
            &storage,
            Some(temp.path()),
            "readSnapshot get_snapshot App.svelte invoke",
            5,
            &HashSet::new(),
        )
        .expect("repo text scan");

        assert!(
            scan.hits
                .first()
                .is_some_and(|hit| hit.display_name.ends_with("App.svelte")),
            "Svelte command surface should rank first: {:#?}",
            scan.hits
        );
    }

    #[test]
    fn repo_text_partial_matches_surface_public_page_wiring() {
        let temp = tempdir().expect("temp dir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
        let page_path = temp
            .path()
            .join("src")
            .join("app")
            .join("(frontend)")
            .join("posts")
            .join("[slug]")
            .join("page.tsx");
        let social_path = temp.path().join("src").join("lib").join("social-feed.ts");
        std::fs::create_dir_all(page_path.parent().expect("page parent")).expect("create page dir");
        std::fs::create_dir_all(social_path.parent().expect("social parent"))
            .expect("create social dir");
        std::fs::write(
            &page_path,
            "import { PostComments } from './PostComments';\nexport default async function PostPage() { return <PostComments />; }\n",
        )
        .expect("write page");
        std::fs::write(
            &social_path,
            "export async function getElsewhereFeed() { return []; }\n",
        )
        .expect("write social feed");

        {
            let storage = Storage::open(&storage_path).expect("open storage");
            for (id, path, language) in [(11, page_path, "tsx"), (12, social_path, "typescript")] {
                storage
                    .insert_file(&FileInfo {
                        id,
                        path,
                        language: language.to_string(),
                        modification_time: 1,
                        indexed: true,
                        complete: true,
                        line_count: 2,
                        file_role: codestory_store::FileRole::Source,
                    })
                    .expect("insert file");
            }
        }

        let storage = Storage::open(&storage_path).expect("reopen storage");
        let scan = AppController::collect_repo_text_hits(
            &storage,
            Some(temp.path()),
            "how posts comments auth and elsewhere feed connect to public pages",
            10,
            &HashSet::new(),
        )
        .expect("repo text scan");

        assert!(
            scan.hits.iter().any(|hit| hit
                .display_name
                .ends_with("src/app/(frontend)/posts/[slug]/page.tsx")),
            "natural-language repo text should surface public page wiring, not only symbols: {:#?}",
            scan.hits
        );
    }

    #[test]
    fn repo_text_partial_match_requires_distinct_query_terms() {
        let temp = tempdir().expect("temp dir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
        let page_path = temp.path().join("src").join("posts").join("page.tsx");
        std::fs::create_dir_all(page_path.parent().expect("page parent")).expect("create page dir");
        std::fs::write(&page_path, "export const posts = [];\n").expect("write page");

        {
            let storage = Storage::open(&storage_path).expect("open storage");
            storage
                .insert_file(&FileInfo {
                    id: 11,
                    path: page_path,
                    language: "tsx".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: codestory_store::FileRole::Source,
                })
                .expect("insert file");
        }

        let storage = Storage::open(&storage_path).expect("reopen storage");
        let scan = AppController::collect_repo_text_hits(
            &storage,
            Some(temp.path()),
            "posts comments auth",
            10,
            &HashSet::new(),
        )
        .expect("repo text scan");

        assert!(
            scan.hits.is_empty(),
            "one repeated term in path and file contents should not satisfy multi-concept repo-text matching: {:#?}",
            scan.hits
        );
    }

    #[test]
    fn repo_text_scan_reports_file_cap_on_large_low_match_fixture() {
        let temp = tempdir().expect("temp dir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
        let src = temp.path().join("src");
        std::fs::create_dir_all(&src).expect("create src");

        {
            let storage = Storage::open(&storage_path).expect("open storage");
            for idx in 0..(REPO_TEXT_SCAN_FILE_CAP + 3) {
                let path = src.join(format!("file_{idx}.rs"));
                std::fs::write(&path, format!("pub fn file_{idx}() {{}}\n"))
                    .expect("write fixture file");
                storage
                    .insert_file(&FileInfo {
                        id: idx as i64 + 1,
                        path,
                        language: "rust".to_string(),
                        modification_time: 1,
                        indexed: true,
                        complete: true,
                        line_count: 1,
                        file_role: codestory_store::FileRole::Source,
                    })
                    .expect("insert file");
            }
        }

        let storage = Storage::open(&storage_path).expect("reopen storage");
        let scan = AppController::collect_repo_text_hits(
            &storage,
            Some(temp.path()),
            "needle that is not present",
            10,
            &HashSet::new(),
        )
        .expect("repo text scan");

        assert!(scan.hits.is_empty());
        assert!(scan.stats.truncated, "{:?}", scan.stats);
        assert!(scan.stats.scanned_file_count <= REPO_TEXT_SCAN_FILE_CAP as u32);
        assert!(
            scan.stats
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("scanning") || reason.contains("ms"))
        );
        assert!(scan.stats.action.is_some());
    }

    #[test]
    fn repo_text_scan_file_cap_sets_truncated_reason() {
        let mut stats = RepoTextScanStatsDto {
            scanned_file_count: REPO_TEXT_SCAN_FILE_CAP as u32,
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

        assert!(AppController::repo_text_scan_should_stop(
            &mut stats,
            &Instant::now()
        ));
        assert!(stats.truncated);
        assert!(
            stats
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("scanning 2000 files")),
            "{stats:?}"
        );
        assert!(stats.action.is_some());
    }

    #[test]
    fn repo_text_scan_skips_large_files_before_reading_contents() {
        let temp = tempdir().expect("temp dir");
        let storage_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
        let source_path = temp.path().join("large.rs");
        std::fs::write(
            &source_path,
            format!(
                "needle\n{}",
                "x".repeat(REPO_TEXT_MAX_FILE_BYTES as usize + 16)
            ),
        )
        .expect("write large source");

        {
            let storage = Storage::open(&storage_path).expect("open storage");
            storage
                .insert_file(&FileInfo {
                    id: 1,
                    path: source_path,
                    language: "rust".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: codestory_store::FileRole::Source,
                })
                .expect("insert file");
        }

        let storage = Storage::open(&storage_path).expect("reopen storage");
        let scan = AppController::collect_repo_text_hits(
            &storage,
            Some(temp.path()),
            "needle",
            10,
            &HashSet::new(),
        )
        .expect("repo text scan");

        assert!(scan.hits.is_empty());
        assert_eq!(scan.stats.scanned_file_count, 1);
        assert_eq!(scan.stats.scanned_byte_count, 0);
        assert_eq!(scan.stats.skipped_large_file_count, 1);
        assert!(!scan.stats.truncated);
    }

    #[test]
    fn direct_markdown_snippet_is_byte_capped() {
        let text = (0..10_000)
            .map(|idx| format!("line {idx}: {}", "x".repeat(2_048)))
            .collect::<Vec<_>>()
            .join("\n");

        let snippet = bounded_direct_markdown_snippet(&text, Some(5_000), usize::MAX);

        assert!(snippet.truncated);
        assert!(snippet.markdown.len() <= DIRECT_SNIPPET_MAX_BYTES);
        assert!(
            snippet.markdown.contains("snippet truncated by byte cap"),
            "{}",
            snippet.markdown
        );
        assert!(
            snippet.markdown.ends_with("```"),
            "truncated snippet should keep a balanced closing fence:\n{}",
            snippet.markdown
        );
    }

    #[test]
    fn file_backed_snippet_streams_and_caps_long_lines() {
        let temp = tempdir().expect("temp dir");
        let source_path = temp.path().join("long_line.rs");
        std::fs::write(
            &source_path,
            format!("pub fn alpha() {{}}\n// {}\n", "x".repeat(256 * 1024)),
        )
        .expect("write long line source");

        let snippet = bounded_markdown_snippet_from_path(
            &source_path,
            2,
            1,
            DIRECT_SNIPPET_MAX_BYTES,
            DIRECT_SNIPPET_TRUNCATION_SUFFIX,
        )
        .expect("read bounded snippet");

        assert!(snippet.truncated);
        assert!(snippet.markdown.len() <= DIRECT_SNIPPET_MAX_BYTES);
        assert!(snippet.markdown.ends_with("```"));
    }

    #[test]
    fn symbol_context_by_id_does_not_mutate_persisted_semantic_docs() {
        let _env = hybrid_test_env();
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index without runtime refresh");

        let storage = Storage::open(&storage_path).expect("reopen storage");
        let before = storage
            .get_llm_symbol_doc_stats()
            .expect("semantic doc stats before");
        let symbol_id = storage
            .get_nodes()
            .expect("load nodes")
            .into_iter()
            .find(|node| {
                matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                    && terminal_symbol_segment(&node_display_name(node)) == "check_winner"
            })
            .map(|node| NodeId::from(node.id))
            .expect("check_winner symbol node");
        drop(storage);

        let context = controller
            .symbol_context(symbol_id.clone())
            .expect("symbol context by id");
        assert_eq!(context.node.id, symbol_id);
        assert!(context.node.display_name.contains("check_winner"));

        let storage = Storage::open(&storage_path).expect("reopen storage after read");
        let after = storage
            .get_llm_symbol_doc_stats()
            .expect("semantic doc stats after");
        assert_eq!(after.doc_count, before.doc_count);
        assert_eq!(after.embedding_model, before.embedding_model);
    }

    #[test]
    fn rebuild_search_state_rebuilds_mixed_model_docs() {
        let temp = tempdir().expect("create temp dir");
        let file_path = write_semantic_fixture(temp.path());
        let mut storage = Storage::new_in_memory().expect("storage");
        insert_semantic_fixture_nodes(&mut storage, &file_path);

        let mut env = hybrid_test_env();
        env.push(EnvGuard::set(EMBEDDING_MODEL_ID_ENV, "model-a"));
        rebuild_search_state_from_storage(&mut storage, temp.path(), None, true)
            .expect("initial rebuild");
        assert_eq!(
            storage
                .get_llm_symbol_doc_stats()
                .expect("initial doc stats")
                .embedding_model
                .as_deref(),
            Some("model-a")
        );
        let mut seeded_docs = storage
            .get_all_llm_symbol_docs()
            .expect("initial semantic docs");
        if seeded_docs.len() == 1 {
            let mut extra = seeded_docs[0].clone();
            extra.node_id = CoreNodeId(3);
            extra.display_name = "beta".to_string();
            extra.qualified_name = Some("pkg::beta".to_string());
            extra.dense_reason = Some("documented_nontrivial".to_string());
            storage
                .upsert_llm_symbol_docs_batch(&[extra])
                .expect("seed second dense doc");
            seeded_docs = storage
                .get_all_llm_symbol_docs()
                .expect("seeded semantic docs");
        }
        let mixed_node_id = seeded_docs
            .last()
            .expect("at least one semantic doc")
            .node_id
            .0;

        storage
            .get_connection()
            .execute(
                "UPDATE llm_symbol_doc
                 SET embedding_model = CASE
                     WHEN node_id = ?1 THEN 'model-b'
                     ELSE embedding_model
                 END",
                [mixed_node_id],
            )
            .expect("mark one semantic doc as mixed");
        assert_eq!(
            storage
                .get_llm_symbol_doc_stats()
                .expect("mixed doc stats")
                .embedding_model,
            None
        );

        env.push(EnvGuard::set(EMBEDDING_MODEL_ID_ENV, "model-b"));
        rebuild_search_state_from_storage(&mut storage, temp.path(), None, true)
            .expect("mixed corpus should force rebuild");

        let docs = storage
            .get_all_llm_symbol_docs()
            .expect("reloaded semantic docs");
        assert!(!docs.is_empty(), "expected rebuilt semantic docs");
        assert!(
            docs.iter().all(|doc| doc.embedding_model == "model-b"),
            "expected mixed semantic docs to be rebuilt to a uniform model"
        );
    }

    #[test]
    fn merge_search_hits_by_node_id_keeps_stronger_expanded_score() {
        let mut hits = vec![
            SearchHit {
                node_id: NodeId("primary".to_string()),
                display_name: "alpha".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some("src/lib.rs".to_string()),
                line: Some(10),
                score: 0.25,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: None,
                score_breakdown: None,
            },
            SearchHit {
                node_id: NodeId("secondary".to_string()),
                display_name: "alpha".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some("src/lib.rs".to_string()),
                line: Some(20),
                score: 0.75,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: None,
                score_breakdown: None,
            },
        ];

        merge_search_hits_by_node_id(
            &mut hits,
            vec![SearchHit {
                node_id: NodeId("primary".to_string()),
                display_name: "alpha".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some("src/lib.rs".to_string()),
                line: Some(10),
                score: 250.0,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: None,
                score_breakdown: None,
            }],
        );

        hits.sort_by(|left, right| compare_search_hits("alpha", left, right));

        assert_eq!(hits[0].node_id, NodeId("primary".to_string()));
        assert_eq!(hits[0].score, 250.0);
    }

    #[test]
    fn embedded_exact_symbol_terms_count_and_annotate_exact_hits() {
        let mut hit = SearchHit {
            node_id: NodeId("search-hybrid".to_string()),
            display_name: "SearchEngine::search_hybrid_with_scores".to_string(),
            kind: codestory_contracts::api::NodeKind::METHOD,
            file_path: Some("src/search/engine.rs".to_string()),
            line: Some(1769),
            score: 0.25,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        };
        let query = "exact symbol first semantic ranking search_hybrid_with_scores";

        assert_eq!(exact_symbol_hit_count(query, std::slice::from_ref(&hit)), 1);

        annotate_search_hit_match_quality(query, std::slice::from_mut(&mut hit));

        assert_eq!(
            hit.match_quality,
            Some(codestory_contracts::api::SearchMatchQualityDto::NormalizedExact)
        );
    }

    #[test]
    fn primary_source_retention_keeps_short_precise_windows() {
        assert_eq!(primary_source_retention_threshold(1), 1);
        assert_eq!(primary_source_retention_threshold(3), 3);
        assert_eq!(primary_source_retention_threshold(10), 3);
        assert_eq!(primary_source_retention_threshold(50), 3);
    }

    #[test]
    fn inexact_search_results_deduplicate_repeated_display_keys() {
        let mut hits = vec![
            SearchHit {
                node_id: NodeId("llamacpp-url-env".to_string()),
                display_name: "LLAMACPP_EMBEDDINGS_URL_ENV".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some("src/search/engine.rs".to_string()),
                line: Some(178),
                score: 0.90,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: None,
                score_breakdown: None,
            },
            SearchHit {
                node_id: NodeId("llamacpp-url-env-copy".to_string()),
                display_name: "LLAMACPP_EMBEDDINGS_URL_ENV".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some("src/search/engine.rs".to_string()),
                line: Some(187),
                score: 0.80,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: None,
                score_breakdown: None,
            },
            SearchHit {
                node_id: NodeId("endpoint-parser".to_string()),
                display_name: "LlamaCppEndpoint::parse".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some("src/search/engine.rs".to_string()),
                line: Some(194),
                score: 0.70,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: None,
                score_breakdown: None,
            },
        ];

        hits.sort_by(|left, right| {
            compare_search_hits(
                "llama.cpp embeddings endpoint URL environment variable configuration",
                left,
                right,
            )
        });
        dedupe_inexact_search_hits_by_display_key(
            "llama.cpp embeddings endpoint URL environment variable configuration",
            &mut hits,
        );

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].node_id, NodeId("endpoint-parser".to_string()));
        assert_eq!(hits[1].node_id, NodeId("llamacpp-url-env".to_string()));
    }

    #[test]
    fn exact_search_results_keep_repeated_display_keys() {
        let mut hits = vec![
            SearchHit {
                node_id: NodeId("llamacpp-url-env".to_string()),
                display_name: "LLAMACPP_EMBEDDINGS_URL_ENV".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some("src/search/engine.rs".to_string()),
                line: Some(178),
                score: 0.90,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: None,
                score_breakdown: None,
            },
            SearchHit {
                node_id: NodeId("llamacpp-url-env-copy".to_string()),
                display_name: "LLAMACPP_EMBEDDINGS_URL_ENV".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some("src/search/engine.rs".to_string()),
                line: Some(187),
                score: 0.80,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                match_quality: None,
                resolvable: true,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: None,
                score_breakdown: None,
            },
        ];

        dedupe_inexact_search_hits_by_display_key("LLAMACPP_EMBEDDINGS_URL_ENV", &mut hits);

        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn exact_symbol_queries_skip_primary_source_pretruncate() {
        assert!(
            !should_pretruncate_primary_source_window("StorageAccess", true, 250, 10),
            "exact symbol queries need final exact-symbol sorting before truncation"
        );
        assert!(should_pretruncate_primary_source_window(
            "how search ranking works",
            true,
            250,
            10
        ));
        assert!(!should_pretruncate_primary_source_window(
            "how search ranking works",
            false,
            250,
            10
        ));
    }

    #[test]
    fn exact_symbol_fast_path_is_conservative() {
        let req = |query: &str,
                   hybrid_weights: Option<AgentHybridWeightsDto>,
                   hybrid_limits: Option<SearchHybridLimitsDto>| SearchRequest {
            query: query.to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights,
            hybrid_limits,
        };

        assert!(exact_symbol_lexical_fast_path(
            &req("Workbench", None, None),
            None
        ));
        assert!(exact_symbol_lexical_fast_path(
            &req("Subcommand::Exec", None, None),
            None
        ));
        assert!(exact_symbol_lexical_fast_path(
            &req("check_winner", None, None),
            None
        ));
        assert!(!exact_symbol_lexical_fast_path(
            &req("authorization", None, None),
            None
        ));
        assert!(!exact_symbol_lexical_fast_path(
            &req("how ExtensionService starts", None, None),
            None
        ));
        assert!(!exact_symbol_lexical_fast_path(
            &req(
                "Workbench",
                None,
                Some(SearchHybridLimitsDto {
                    lexical: None,
                    semantic: Some(20),
                }),
            ),
            None
        ));

        let weights = AgentHybridWeightsDto {
            lexical: Some(0.25),
            semantic: Some(0.75),
            graph: None,
        };
        assert!(!exact_symbol_lexical_fast_path(
            &req("Workbench", Some(weights.clone()), None),
            Some(&weights)
        ));
    }

    #[test]
    fn exact_symbol_merged_lexical_queries_dedupe_exact_anchor_scan() {
        assert_eq!(
            exact_symbol_merged_lexical_queries("Workbench"),
            vec!["Workbench".to_string()]
        );
        assert_eq!(
            exact_symbol_merged_lexical_queries("Subcommand::Exec"),
            vec!["Subcommand::Exec".to_string(), "Exec".to_string()]
        );
        assert_eq!(
            exact_symbol_merged_lexical_queries("how ExtensionHostManager starts"),
            vec!["how ExtensionHostManager starts".to_string()]
        );
    }

    #[test]
    fn mixed_natural_language_query_detects_embedded_symbol_prompts() {
        assert!(mixed_natural_language_query(
            "how ExtensionHostManager starts"
        ));
        assert!(!mixed_natural_language_query("Workbench"));
        assert!(!mixed_natural_language_query("Subcommand::Exec"));
    }

    #[test]
    fn hybrid_search_config_skips_exact_symbol_escalation_for_mixed_nl() {
        let req = SearchRequest {
            query: "how ExtensionHostManager starts".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        };
        let config = hybrid_search_config_for_request(&req, 10, None, true);
        assert_eq!(config.max_results, 10);
    }

    #[test]
    fn exact_symbol_fast_path_returns_lexical_hits_without_semantic_fallback() {
        let mut engine = SearchEngine::new(None).expect("search engine");
        engine
            .index_nodes(vec![(CoreNodeId(1), "Workbench".to_string())])
            .expect("index nodes");
        let req = SearchRequest {
            query: "Workbench".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        };
        let storage_retrieval = RetrievalStateDto {
            mode: RetrievalModeDto::Hybrid,
            hybrid_configured: true,
            semantic_ready: true,
            semantic_mode: SemanticModeDto::Enabled,
            semantic_doc_count: 170_000,
            embedding_model: Some("test-model".to_string()),
            current_embedding: None,
            stored_embedding: None,
            fallback_reason: None,
            fallback_message: None,
        };
        let graph_boosts = HashMap::new();
        let mut retrieval = storage_retrieval.clone();
        let use_exact_symbol_lexical_fast_path = exact_symbol_lexical_fast_path(&req, None);

        let hits = hybrid_hits_for_retrieval_state(
            &mut engine,
            HybridHitsContext {
                req: &req,
                graph_boosts: &graph_boosts,
                requested_max_results: 10,
                request_weights: None,
                prefer_primary_sources: true,
                storage_retrieval: &storage_retrieval,
                use_exact_symbol_lexical_fast_path,
            },
            &mut retrieval,
        );

        assert!(use_exact_symbol_lexical_fast_path);
        assert_eq!(hits.first().map(|hit| hit.node_id), Some(CoreNodeId(1)));
        assert_eq!(hits[0].semantic_score, 0.0);
        assert_eq!(retrieval.fallback_reason, None);
        assert_eq!(retrieval.fallback_message, None);
    }

    #[test]
    fn zero_semantic_request_weights_use_lexical_hits_without_semantic_fallback() {
        let mut engine = SearchEngine::new(None).expect("search engine");
        engine
            .index_nodes(vec![(CoreNodeId(1), "ExtensionHostManager".to_string())])
            .expect("index nodes");
        let req = SearchRequest {
            query: "ExtensionHostManager".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        };
        let storage_retrieval = RetrievalStateDto {
            mode: RetrievalModeDto::Hybrid,
            hybrid_configured: true,
            semantic_ready: true,
            semantic_mode: SemanticModeDto::Enabled,
            semantic_doc_count: 170_000,
            embedding_model: Some("test-model".to_string()),
            current_embedding: None,
            stored_embedding: None,
            fallback_reason: None,
            fallback_message: None,
        };
        let graph_boosts = HashMap::new();
        let mut retrieval = storage_retrieval.clone();
        let request_weights = AgentHybridWeightsDto {
            lexical: Some(1.0),
            semantic: Some(0.0),
            graph: Some(0.0),
        };

        let hits = hybrid_hits_for_retrieval_state(
            &mut engine,
            HybridHitsContext {
                req: &req,
                graph_boosts: &graph_boosts,
                requested_max_results: 10,
                request_weights: Some(request_weights),
                prefer_primary_sources: true,
                storage_retrieval: &storage_retrieval,
                use_exact_symbol_lexical_fast_path: false,
            },
            &mut retrieval,
        );

        assert_eq!(hits.first().map(|hit| hit.node_id), Some(CoreNodeId(1)));
        assert_eq!(hits[0].semantic_score, 0.0);
        assert_eq!(retrieval.fallback_reason, None);
        assert_eq!(retrieval.fallback_message, None);
    }

    #[test]
    fn exact_symbol_merged_lexical_hits_include_terminal_symbol_matches() {
        let mut engine = SearchEngine::new(None).expect("search engine");
        engine
            .index_nodes(vec![
                (CoreNodeId(1), "exec_events::ThreadEvent".to_string()),
                (CoreNodeId(2), "ThreadEvent".to_string()),
                (
                    CoreNodeId(3),
                    "crate::exec_events::ThreadEvent (import)".to_string(),
                ),
            ])
            .expect("index nodes");

        let hits = exact_symbol_merged_lexical_hybrid_hits(
            &engine,
            "exec_events::ThreadEvent",
            &HashMap::new(),
        );
        let ids = hits.iter().map(|hit| hit.node_id).collect::<Vec<_>>();

        assert!(
            ids.contains(&CoreNodeId(2)),
            "terminal exact symbol should be admitted beside qualified aliases: {ids:?}"
        );
        assert_eq!(
            ids.iter().filter(|id| **id == CoreNodeId(2)).count(),
            1,
            "exact-symbol merging should preserve node uniqueness: {ids:?}"
        );
    }

    #[test]
    fn full_index_rebuilds_semantic_docs_when_source_text_changes() {
        let _env = hybrid_test_env();
        let workspace = tempdir().expect("workspace dir");
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        write_reindex_semantic_fixture(workspace.path(), "initial compressed digest");
        controller
            .open_project_with_storage_path(workspace.path().to_path_buf(), storage_path.clone())
            .expect("open project");
        controller
            .run_indexing_blocking(IndexMode::Full)
            .expect("initial full index");

        let storage = Storage::open(&storage_path).expect("open storage after initial index");
        let initial_docs = storage
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("load initial symbol docs")
            .into_iter()
            .filter(|doc| doc.display_name == "build_snapshot_digest")
            .collect::<Vec<_>>();
        assert!(!initial_docs.is_empty(), "initial digest doc");
        assert!(
            initial_docs
                .iter()
                .any(|doc| doc.doc_text.contains("initial_compressed_digest")),
            "initial digest docs should include fixture source text: {:?}",
            initial_docs
                .iter()
                .map(|doc| doc.doc_text.as_str())
                .collect::<Vec<_>>()
        );
        drop(storage);

        write_reindex_semantic_fixture(workspace.path(), "updated compressed digest");
        controller
            .run_indexing_blocking(IndexMode::Full)
            .expect("rerun full index");

        let storage = Storage::open(&storage_path).expect("open storage after rerun");
        let updated_docs = storage
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("load updated symbol docs")
            .into_iter()
            .filter(|doc| doc.display_name == "build_snapshot_digest")
            .collect::<Vec<_>>();
        assert!(!updated_docs.is_empty(), "updated digest doc");
        assert!(
            updated_docs
                .iter()
                .any(|doc| doc.doc_text.contains("updated_compressed_digest")),
            "updated digest docs should include fixture source text: {:?}",
            updated_docs
                .iter()
                .map(|doc| doc.doc_text.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            !updated_docs
                .iter()
                .any(|doc| doc.doc_text.contains("initial_compressed_digest")),
            "full index should rebuild symbol docs instead of reusing stale persisted content"
        );
    }

    #[test]
    fn finalize_indexing_without_runtime_refresh_propagates_rebuild_failure() {
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");

        {
            let mut state = controller.state.lock();
            state.is_indexing = true;
            state
                .node_names
                .insert(CoreNodeId(999), "stale_symbol".to_string());
            let engine = SearchEngine::new(None).expect("search engine");
            publish_search_engine(&mut state, engine);
        }

        let error = controller
            .finalize_indexing_without_runtime_refresh_with(&storage_path, None, |_storage, _| {
                Err(ApiError::internal("forced rebuild failure".to_string()))
            })
            .expect_err("forced rebuild failure should propagate");

        assert_eq!(error.code, "internal");
        assert_eq!(error.message, "forced rebuild failure");

        let state = controller.state.lock();
        assert!(!state.is_indexing);
        assert!(state.search_engine.is_none());
        assert!(state.node_names.is_empty());
    }

    #[test]
    fn blocking_index_without_open_project_does_not_leave_indexing_stuck() {
        let controller = AppController::new();

        let error = controller
            .run_indexing_blocking(IndexMode::Full)
            .expect_err("missing project should error");

        assert_eq!(error.code, "invalid_argument");
        assert!(!controller.state.lock().is_indexing);
    }

    #[test]
    fn search_rejects_reads_while_indexing_is_active() {
        let controller = AppController::new();
        {
            let mut state = controller.state.lock();
            state.is_indexing = true;
        }

        let error = controller
            .search_results(SearchRequest {
                query: "check_winner".to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 10,
                expand_search_plan: false,
                hybrid_weights: None,
                hybrid_limits: None,
            })
            .expect_err("search should be blocked while indexing");

        assert_eq!(error.code, "invalid_argument");
        assert!(error.message.contains("indexing is in progress"));
    }

    #[test]
    fn search_after_summary_open_stays_sidecar_primary_without_runtime_refresh() {
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(workspace.path().to_path_buf(), storage_path)
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index without runtime refresh");

        let error = controller
            .search(SearchRequest {
                query: "check_winner".to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 10,
                expand_search_plan: false,
                hybrid_weights: None,
                hybrid_limits: None,
            })
            .expect_err("search should require full sidecars after summary open");

        assert_mandatory_sidecar_unavailable(&error);
        let state = controller.state.lock();
        assert!(state.search_engine.is_none());
        assert!(state.node_names.is_empty());
    }

    #[test]
    fn full_refresh_returns_with_summary_ready_and_detail_dirty() {
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index without runtime refresh");

        let storage = Storage::open(&storage_path).expect("reopen storage");
        assert!(
            storage
                .snapshots()
                .has_ready_summary()
                .expect("summary snapshot readiness"),
            "full refresh should publish ready grounding summary snapshots"
        );
        assert!(
            !storage
                .snapshots()
                .has_ready_detail()
                .expect("detail snapshot readiness"),
            "full refresh should leave grounding detail snapshots dirty"
        );
    }

    #[test]
    fn normalized_hybrid_weights_clamps_and_normalizes_values() {
        let fallback = HybridSearchConfig::default();
        let (lexical, semantic, graph) = normalized_hybrid_weights(
            Some(AgentHybridWeightsDto {
                lexical: Some(2.0),
                semantic: Some(-1.0),
                graph: Some(0.5),
            }),
            &fallback,
        );

        assert!((lexical - 0.666_666_7).abs() < 1e-4);
        assert!((semantic - 0.0).abs() < 1e-6);
        assert!((graph - 0.333_333_34).abs() < 1e-4);
    }

    #[test]
    fn normalized_hybrid_weights_falls_back_when_invalid_sum() {
        let fallback = HybridSearchConfig::default();
        let (lexical, semantic, graph) = normalized_hybrid_weights(
            Some(AgentHybridWeightsDto {
                lexical: Some(0.0),
                semantic: Some(0.0),
                graph: Some(0.0),
            }),
            &fallback,
        );

        assert!((lexical - fallback.lexical_weight).abs() < 1e-6);
        assert!((semantic - fallback.semantic_weight).abs() < 1e-6);
        assert!((graph - fallback.graph_weight).abs() < 1e-6);
    }

    #[test]
    fn hybrid_search_defaults_to_accuracy_first_semantic_profile() {
        let config = HybridSearchConfig::default();

        assert_eq!(config.max_results, 20);
        assert_eq!(config.lexical_weight, 0.0);
        assert_eq!(config.semantic_weight, 1.0);
        assert_eq!(config.graph_weight, 0.0);
        assert_eq!(config.lexical_limit, 0);
        assert_eq!(config.semantic_limit, 20);
    }

    #[test]
    fn apply_hybrid_limits_overrides_and_caps_values() {
        let mut config = HybridSearchConfig::default();
        apply_hybrid_limits(
            Some(codestory_contracts::api::SearchHybridLimitsDto {
                lexical: Some(0),
                semantic: Some(5_000),
            }),
            &mut config,
        );

        assert_eq!(config.lexical_limit, 0);
        assert_eq!(config.semantic_limit, 1_000);
    }

    #[test]
    fn progress_forwarder_relays_progress_and_status_events() {
        let (event_tx, event_rx) = unbounded::<Event>();
        let (app_tx, app_rx) = unbounded::<AppEventPayload>();
        let handle = spawn_progress_forwarder(event_rx, app_tx);

        event_tx
            .send(Event::IndexingProgress {
                current: 3,
                total: 5,
            })
            .expect("send progress event");
        event_tx
            .send(Event::StatusUpdate {
                message: "ignore me".to_string(),
            })
            .expect("send status event");
        drop(event_tx);

        let forwarded = app_rx.recv().expect("receive forwarded event");
        assert!(matches!(
            forwarded,
            AppEventPayload::IndexingProgress {
                current: 3,
                total: 5
            }
        ));
        let status = app_rx.recv().expect("receive status update");
        assert!(matches!(
            status,
            AppEventPayload::StatusUpdate { message } if message == "ignore me"
        ));
        assert!(
            app_rx.try_recv().is_err(),
            "unexpected extra forwarded events"
        );
        handle.join().expect("join forwarder");
    }

    #[test]
    fn write_file_text_writes_inside_project_root() {
        let temp = tempdir().expect("create temp dir");
        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let result = controller
            .write_file_text(WriteFileTextRequest {
                path: "notes.txt".to_string(),
                text: "hello world".to_string(),
            })
            .expect("write text file");

        assert_eq!(result.bytes_written, 11);
        let saved = std::fs::read_to_string(temp.path().join("notes.txt")).expect("read file");
        assert_eq!(saved, "hello world");
    }

    #[test]
    fn write_file_text_rejects_paths_outside_project_root() {
        let temp = tempdir().expect("create temp dir");
        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let err = controller
            .write_file_text(WriteFileTextRequest {
                path: "../escape.txt".to_string(),
                text: "nope".to_string(),
            })
            .expect_err("write should fail");

        assert_eq!(err.code, "invalid_argument");
    }

    #[test]
    fn list_root_symbols_deduplicates_repeated_entries() {
        let temp = tempdir().expect("create temp dir");
        let db_path = temp.path().join("codestory.db");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(101),
                        kind: NodeKind::MODULE,
                        serialized_name: "\"react\"".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(102),
                        kind: NodeKind::MODULE,
                        serialized_name: "\"react\"".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(103),
                        kind: NodeKind::MODULE,
                        serialized_name: "\"./app/types\"".to_string(),
                        ..Default::default()
                    },
                ])
                .expect("insert root nodes");
        }

        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let roots = controller
            .list_root_symbols(ListRootSymbolsRequest { limit: None })
            .expect("load roots");
        let react_count = roots
            .iter()
            .filter(|symbol| symbol.label == "\"react\"")
            .count();

        assert_eq!(react_count, 1);
        assert!(roots.iter().any(|symbol| symbol.label == "\"./app/types\""));
    }

    #[test]
    fn graph_neighborhood_member_includes_owner_inheritance_edges() {
        let temp = tempdir().expect("create temp dir");
        let db_path = temp.path().join("codestory.db");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(1),
                        kind: NodeKind::INTERFACE,
                        serialized_name: "EventListener".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(2),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "EventListener::handle_event".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(3),
                        kind: NodeKind::CLASS,
                        serialized_name: "UiListener".to_string(),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
            storage
                .insert_edges_batch(&[
                    Edge {
                        id: EdgeId(11),
                        source: CoreNodeId(1),
                        target: CoreNodeId(2),
                        kind: EdgeKind::MEMBER,
                        ..Default::default()
                    },
                    Edge {
                        id: EdgeId(12),
                        source: CoreNodeId(3),
                        target: CoreNodeId(1),
                        kind: EdgeKind::INHERITANCE,
                        ..Default::default()
                    },
                ])
                .expect("insert edges");
        }

        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let graph = controller
            .graph_neighborhood(GraphRequest {
                center_id: codestory_contracts::api::NodeId("2".to_string()),
                max_edges: None,
            })
            .expect("load graph neighborhood");

        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.kind == codestory_contracts::api::EdgeKind::INHERITANCE),
            "Expected INHERITANCE edge from owner trait context"
        );
        assert!(
            graph.canonical_layout.is_some(),
            "Expected canonical_layout on neighborhood response"
        );
    }

    #[test]
    fn graph_trail_includes_canonical_layout() {
        let temp = tempdir().expect("create temp dir");
        let db_path = temp.path().join("codestory.db");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(1),
                        kind: NodeKind::CLASS,
                        serialized_name: "Runner".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(2),
                        kind: NodeKind::METHOD,
                        serialized_name: "Runner::run".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(3),
                        kind: NodeKind::METHOD,
                        serialized_name: "Worker::execute".to_string(),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
            storage
                .insert_edges_batch(&[
                    Edge {
                        id: EdgeId(11),
                        source: CoreNodeId(1),
                        target: CoreNodeId(2),
                        kind: EdgeKind::MEMBER,
                        ..Default::default()
                    },
                    Edge {
                        id: EdgeId(12),
                        source: CoreNodeId(2),
                        target: CoreNodeId(3),
                        kind: EdgeKind::CALL,
                        ..Default::default()
                    },
                ])
                .expect("insert edges");
        }

        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let graph = controller
            .graph_trail(TrailConfigDto {
                root_id: codestory_contracts::api::NodeId("2".to_string()),
                mode: codestory_contracts::api::TrailMode::Neighborhood,
                target_id: None,
                depth: 2,
                direction: codestory_contracts::api::TrailDirection::Both,
                caller_scope: codestory_contracts::api::TrailCallerScope::ProductionOnly,
                edge_filter: vec![],
                show_utility_calls: false,
                hide_speculative: false,
                story: false,
                node_filter: vec![],
                max_nodes: 128,
                layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
            })
            .expect("load graph trail");

        assert!(
            graph.canonical_layout.is_some(),
            "Expected canonical_layout on trail response"
        );
    }

    #[test]
    fn graph_direct_references_returns_filtered_direct_incoming_edges() {
        let temp = tempdir().expect("create temp dir");
        let db_path = temp.path().join("codestory.db");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(10),
                        kind: NodeKind::FILE,
                        serialized_name: "src/lib.rs".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(11),
                        kind: NodeKind::FILE,
                        serialized_name: "tests/lib_test.rs".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(1),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "target".to_string(),
                        file_node_id: Some(CoreNodeId(10)),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(2),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "prod_caller".to_string(),
                        file_node_id: Some(CoreNodeId(10)),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(3),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "test_caller".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(4),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "uncertain_caller".to_string(),
                        file_node_id: Some(CoreNodeId(10)),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
            storage
                .insert_edges_batch(&[
                    Edge {
                        id: EdgeId(21),
                        source: CoreNodeId(2),
                        target: CoreNodeId(1),
                        kind: EdgeKind::CALL,
                        file_node_id: Some(CoreNodeId(10)),
                        certainty: Some(ResolutionCertainty::Certain),
                        confidence: Some(0.95),
                        ..Default::default()
                    },
                    Edge {
                        id: EdgeId(22),
                        source: CoreNodeId(3),
                        target: CoreNodeId(1),
                        kind: EdgeKind::CALL,
                        file_node_id: Some(CoreNodeId(11)),
                        certainty: Some(ResolutionCertainty::Certain),
                        confidence: Some(0.95),
                        ..Default::default()
                    },
                    Edge {
                        id: EdgeId(23),
                        source: CoreNodeId(4),
                        target: CoreNodeId(1),
                        kind: EdgeKind::CALL,
                        file_node_id: Some(CoreNodeId(10)),
                        certainty: Some(ResolutionCertainty::Uncertain),
                        confidence: Some(0.4),
                        ..Default::default()
                    },
                ])
                .expect("insert edges");
        }

        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let graph = controller
            .graph_direct_references(TrailConfigDto {
                root_id: codestory_contracts::api::NodeId("1".to_string()),
                mode: codestory_contracts::api::TrailMode::AllReferencing,
                target_id: None,
                depth: 0,
                direction: codestory_contracts::api::TrailDirection::Incoming,
                caller_scope: codestory_contracts::api::TrailCallerScope::ProductionOnly,
                edge_filter: vec![],
                show_utility_calls: false,
                hide_speculative: true,
                story: false,
                node_filter: vec![],
                max_nodes: 10,
                layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
            })
            .expect("load direct references");

        let edge_sources = graph
            .edges
            .iter()
            .map(|edge| edge.source.0.as_str())
            .collect::<Vec<_>>();
        assert_eq!(edge_sources, vec!["2"]);
        let node_ids = graph
            .nodes
            .iter()
            .map(|node| node.id.0.as_str())
            .collect::<Vec<_>>();
        assert_eq!(node_ids, vec!["1", "2"]);
        assert!(graph.canonical_layout.is_none());
    }

    #[test]
    fn high_fanout_graph_trail_reports_truncation_at_max_nodes() {
        let temp = tempdir().expect("create temp dir");
        let db_path = temp.path().join("codestory.db");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            let mut nodes = vec![Node {
                id: CoreNodeId(1),
                kind: NodeKind::FUNCTION,
                serialized_name: "root".to_string(),
                ..Default::default()
            }];
            let mut edges = Vec::new();
            for idx in 2..80 {
                nodes.push(Node {
                    id: CoreNodeId(idx),
                    kind: NodeKind::FUNCTION,
                    serialized_name: format!("child_{idx}"),
                    ..Default::default()
                });
                edges.push(Edge {
                    id: EdgeId(idx + 100),
                    source: CoreNodeId(1),
                    target: CoreNodeId(idx),
                    kind: EdgeKind::CALL,
                    ..Default::default()
                });
            }
            storage.insert_nodes_batch(&nodes).expect("insert nodes");
            storage.insert_edges_batch(&edges).expect("insert edges");
        }

        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let graph = controller
            .graph_trail(TrailConfigDto {
                root_id: codestory_contracts::api::NodeId("1".to_string()),
                mode: codestory_contracts::api::TrailMode::Neighborhood,
                target_id: None,
                depth: 1,
                direction: codestory_contracts::api::TrailDirection::Outgoing,
                caller_scope: codestory_contracts::api::TrailCallerScope::ProductionOnly,
                edge_filter: vec![],
                show_utility_calls: true,
                hide_speculative: false,
                story: false,
                node_filter: vec![],
                max_nodes: 10,
                layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
            })
            .expect("load high fanout trail");

        assert!(graph.truncated, "expected trail truncation: {graph:?}");
        assert!(graph.nodes.len() <= 10);
    }

    #[test]
    fn update_bookmark_category_returns_not_found_when_missing() {
        let temp = tempdir().expect("create temp dir");
        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let err = controller
            .update_bookmark_category(
                9_999,
                UpdateBookmarkCategoryRequest {
                    name: "Renamed".to_string(),
                },
            )
            .expect_err("missing category should return not_found");

        assert_eq!(err.code, "not_found");
    }
}
