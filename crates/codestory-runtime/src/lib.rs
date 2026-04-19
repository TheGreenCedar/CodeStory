use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentBackend, AgentConnectionSettingsDto,
    AgentHybridWeightsDto, ApiError, AppEventPayload, BookmarkCategoryDto, BookmarkDto,
    CreateBookmarkCategoryRequest, CreateBookmarkRequest, EdgeId, EdgeKind, EdgeOccurrencesRequest,
    GraphEdgeDto, GraphNodeDto, GraphRequest, GraphResponse, GroundingBudgetDto,
    GroundingCoverageBucketDto, GroundingFileDigestDto, GroundingSnapshotDto,
    GroundingSymbolDigestDto, IndexMode, IndexingPhaseTimings, ListChildrenSymbolsRequest,
    ListRootSymbolsRequest, MemberAccess, NodeDetailsDto, NodeDetailsRequest, NodeId, NodeKind,
    NodeOccurrencesRequest, OpenContainingFolderRequest, OpenDefinitionRequest, OpenProjectRequest,
    ProjectSummary, ReadFileTextRequest, ReadFileTextResponse, RetrievalFallbackReasonDto,
    RetrievalModeDto, RetrievalStateDto, SearchHit, SearchRepoTextMode, SearchRequest,
    SearchResultsDto, SnippetContextDto, SourceOccurrenceDto, StartIndexingRequest,
    StorageStatsDto, SymbolContextDto, SymbolSummaryDto, SystemActionResponse, TrailConfigDto,
    TrailContextDto, TrailFilterOptionsDto, UpdateBookmarkCategoryRequest, UpdateBookmarkRequest,
    WriteFileResponse, WriteFileTextRequest,
};
use codestory_contracts::events::{Event, EventBus};
use codestory_indexer::IncrementalIndexingStats;
use codestory_indexer::WorkspaceIndexer as V2WorkspaceIndexer;
use codestory_store::{
    FileInfo, GroundingEdgeKindCount, GroundingNodeRecord, LlmSymbolDoc, LlmSymbolDocStats,
    SearchSymbolProjection, SnapshotStore, Store,
};
use codestory_workspace::{
    IndexedFileRecord, RefreshExecutionPlan, RefreshInputs, Workspace, WorkspaceInventory,
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

mod agent;
mod agent_commands;
mod graph_builders;
mod graph_canonical;
mod grounding;
mod mermaid;
mod path_resolution;
mod search;
mod search_runtime;
mod services;
mod support;
mod symbol_query;
mod system_actions;

pub(crate) use agent_commands::{
    agent_backend_label, configured_agent_command, resolve_agent_command,
};
pub use codestory_contracts as contracts;
pub(crate) use mermaid::{fallback_mermaid, mermaid_flowchart, mermaid_gantt, mermaid_sequence};
pub(crate) use search_runtime::SearchEngine;
pub use search_runtime::*;
pub use services::{
    AgentService, GroundingService, IndexService, ProjectService, SearchService, TrailService,
};
pub(crate) use support::{
    FocusedSourceContext, HYBRID_RETRIEVAL_ENABLED_ENV, LocalAgentResponse,
    aggregate_symbol_matches, build_local_agent_prompt, clamp_i64_to_u32, clamp_u64_to_u32,
    clamp_u128_to_u32, clamp_usize_to_u32, extract_symbol_search_terms, file_text_match_line,
    hybrid_retrieval_enabled, looks_like_repo_text_query, node_display_name,
    normalized_hybrid_weights, preferred_occurrence, read_searchable_file_contents,
    should_expand_symbol_query, truncate_for_diagnostic,
};
pub(crate) use symbol_query::compare_search_hits;
pub use symbol_query::{
    SymbolNameMatchRank, compare_ranked_hits, leading_symbol_segment, normalize_symbol_query,
    symbol_name_match_rank, terminal_symbol_segment,
};

type Storage = Store;
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
        let mut telemetry = Self {
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
            resolution_override_count_ms: None,
            resolution_unresolved_counts_ms: None,
            resolution_calls_ms: None,
            resolution_imports_ms: None,
            resolution_cleanup_ms: None,
            resolution_call_candidate_index_ms: None,
            resolution_import_candidate_index_ms: None,
            resolution_call_semantic_index_ms: None,
            resolution_import_semantic_index_ms: None,
            resolution_call_semantic_candidates_ms: None,
            resolution_import_semantic_candidates_ms: None,
            resolution_call_semantic_requests: None,
            resolution_call_semantic_unique_requests: None,
            resolution_call_semantic_skipped_requests: None,
            resolution_import_semantic_requests: None,
            resolution_import_semantic_unique_requests: None,
            resolution_import_semantic_skipped_requests: None,
            resolution_call_compute_ms: None,
            resolution_import_compute_ms: None,
            resolution_call_apply_ms: None,
            resolution_import_apply_ms: None,
            resolution_override_resolution_ms: None,
            resolved_calls_same_file: None,
            resolved_calls_same_module: None,
            resolved_calls_global_unique: None,
            resolved_calls_semantic: None,
            resolved_imports_same_file: None,
            resolved_imports_same_module: None,
            resolved_imports_global_unique: None,
            resolved_imports_fuzzy: None,
            resolved_imports_semantic: None,
        };
        if !index_stats.resolution_ran {
            return telemetry;
        }
        telemetry.resolution_override_count_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_override_count_ms));
        telemetry.resolution_unresolved_counts_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_unresolved_counts_ms,
        ));
        telemetry.resolution_calls_ms = Some(clamp_u64_to_u32(index_stats.resolution_calls_ms));
        telemetry.resolution_imports_ms = Some(clamp_u64_to_u32(index_stats.resolution_imports_ms));
        telemetry.resolution_cleanup_ms = Some(clamp_u64_to_u32(index_stats.resolution_cleanup_ms));
        telemetry.resolution_call_candidate_index_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_call_candidate_index_ms,
        ));
        telemetry.resolution_import_candidate_index_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_import_candidate_index_ms,
        ));
        telemetry.resolution_call_semantic_index_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_call_semantic_index_ms,
        ));
        telemetry.resolution_import_semantic_index_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_import_semantic_index_ms,
        ));
        telemetry.resolution_call_semantic_candidates_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_call_semantic_candidates_ms,
        ));
        telemetry.resolution_import_semantic_candidates_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_import_semantic_candidates_ms,
        ));
        telemetry.resolution_call_semantic_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_call_semantic_requests,
        ));
        telemetry.resolution_call_semantic_unique_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_call_semantic_unique_requests,
        ));
        telemetry.resolution_call_semantic_skipped_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_call_semantic_skipped_requests,
        ));
        telemetry.resolution_import_semantic_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_import_semantic_requests,
        ));
        telemetry.resolution_import_semantic_unique_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_import_semantic_unique_requests,
        ));
        telemetry.resolution_import_semantic_skipped_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_import_semantic_skipped_requests,
        ));
        telemetry.resolution_call_compute_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_call_compute_ms));
        telemetry.resolution_import_compute_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_import_compute_ms));
        telemetry.resolution_call_apply_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_call_apply_ms));
        telemetry.resolution_import_apply_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_import_apply_ms));
        telemetry.resolution_override_resolution_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_override_resolution_ms,
        ));
        telemetry.resolved_calls_same_file =
            Some(clamp_usize_to_u32(index_stats.resolved_calls_same_file));
        telemetry.resolved_calls_same_module =
            Some(clamp_usize_to_u32(index_stats.resolved_calls_same_module));
        telemetry.resolved_calls_global_unique =
            Some(clamp_usize_to_u32(index_stats.resolved_calls_global_unique));
        telemetry.resolved_calls_semantic =
            Some(clamp_usize_to_u32(index_stats.resolved_calls_semantic));
        telemetry.resolved_imports_same_file =
            Some(clamp_usize_to_u32(index_stats.resolved_imports_same_file));
        telemetry.resolved_imports_same_module =
            Some(clamp_usize_to_u32(index_stats.resolved_imports_same_module));
        telemetry.resolved_imports_global_unique = Some(clamp_usize_to_u32(
            index_stats.resolved_imports_global_unique,
        ));
        telemetry.resolved_imports_fuzzy =
            Some(clamp_usize_to_u32(index_stats.resolved_imports_fuzzy));
        telemetry.resolved_imports_semantic =
            Some(clamp_usize_to_u32(index_stats.resolved_imports_semantic));
        telemetry
    }
}

fn parse_db_id(raw: &str, field_name: &str) -> Result<i64, ApiError> {
    raw.trim()
        .parse::<i64>()
        .map_err(|_| ApiError::invalid_argument(format!("Invalid {field_name}: {raw}")))
}

fn edge_certainty_label(
    certainty: Option<codestory_contracts::graph::ResolutionCertainty>,
    confidence: Option<f32>,
) -> Option<String> {
    certainty
        .or_else(|| codestory_contracts::graph::ResolutionCertainty::from_confidence(confidence))
        .map(|value| value.as_str().to_string())
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
            edge_certainty_label(edge.certainty, edge.confidence)
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
    docs_reused: u32,
    docs_embedded: u32,
    docs_pending: u32,
    docs_stale: u32,
}

struct SearchStateBuildResult {
    node_names: HashMap<codestory_contracts::graph::NodeId, String>,
    engine: SearchEngine,
    semantic_stats: SemanticProjectionStats,
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
    timings.semantic_docs_reused = Some(stats.docs_reused);
    timings.semantic_docs_embedded = Some(stats.docs_embedded);
    timings.semantic_docs_pending = Some(stats.docs_pending);
    timings.semantic_docs_stale = Some(stats.docs_stale);
}

fn build_search_state(
    storage: &mut Storage,
    search_storage_path: Option<&Path>,
    nodes: Vec<codestory_contracts::graph::Node>,
    llm_refresh_file_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
    semantic_projection_mode: SemanticProjectionMode,
    hydrate_semantic_docs: bool,
) -> Result<SearchStateBuildResult, ApiError> {
    storage
        .rebuild_search_symbol_projection_from_node_table()
        .map_err(|e| {
            ApiError::internal(format!("Failed to rebuild search symbol projection: {e}"))
        })?;
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

const LLM_SYMBOL_DOC_SCHEMA_VERSION: u32 = 2;
const LLM_SYMBOL_DOC_VERSION_PREFIX: &str = "semantic_doc_version:";
const SEARCH_NODE_BATCH_SIZE: usize = 8_192;
const SEARCH_SYMBOL_PROJECTION_BATCH_SIZE: usize = 4_096;
const LLM_DOC_RELOAD_BATCH_SIZE: usize = 256;
const LLM_DOC_EMBED_BATCH_SIZE: usize = 64;
const LLM_DOC_EMBED_BATCH_SIZE_ENV: &str = "CODESTORY_LLM_DOC_EMBED_BATCH_SIZE";

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
    let mut engine = SearchEngine::new(Some(search_storage_path))
        .map_err(|e| ApiError::internal(format!("Failed to init search engine: {e}")))?;
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
        match SearchEngine::open_existing(search_storage_path.as_path()) {
            Ok(mut engine) => {
                engine.load_symbol_projection(
                    projection
                        .iter()
                        .map(|entry| (entry.node_id, entry.display_name.clone())),
                );
                if engine.full_text_doc_count() != projection.len() {
                    build_search_engine_from_projection(search_storage_path.as_path(), &projection)?
                } else {
                    engine
                }
            }
            Err(error) => {
                tracing::warn!(
                    "Failed to open persisted search index at {}: {}. Rebuilding from projection.",
                    search_storage_path.display(),
                    error
                );
                build_search_engine_from_projection(search_storage_path.as_path(), &projection)?
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
        .map(|value| value.clamp(1, 512))
        .unwrap_or(LLM_DOC_EMBED_BATCH_SIZE)
}

fn retrieval_state_from_parts(
    semantic_doc_count: u32,
    embedding_model: Option<String>,
    embedding_runtime_available: bool,
    fallback_message: Option<String>,
) -> RetrievalStateDto {
    let hybrid_configured = hybrid_retrieval_enabled();
    let fallback_reason = if !hybrid_configured {
        Some(RetrievalFallbackReasonDto::DisabledByConfig)
    } else if !embedding_runtime_available {
        Some(RetrievalFallbackReasonDto::MissingEmbeddingRuntime)
    } else if semantic_doc_count == 0 {
        Some(RetrievalFallbackReasonDto::MissingSemanticDocs)
    } else {
        None
    };
    let semantic_ready = hybrid_configured && embedding_runtime_available && semantic_doc_count > 0;
    let mode = if semantic_ready {
        RetrievalModeDto::Hybrid
    } else {
        RetrievalModeDto::Symbolic
    };
    let fallback_message = fallback_message.or_else(|| match fallback_reason {
        Some(RetrievalFallbackReasonDto::DisabledByConfig) => Some(format!(
            "Hybrid retrieval disabled by {HYBRID_RETRIEVAL_ENABLED_ENV}=false; using symbolic ranking."
        )),
        Some(RetrievalFallbackReasonDto::MissingSemanticDocs) => Some(
            "Semantic assets are available, but semantic symbol docs have not been built yet. Run `index` or reopen the project to populate them."
                .to_string(),
        ),
        _ => None,
    });

    RetrievalStateDto {
        mode,
        hybrid_configured,
        semantic_ready,
        semantic_doc_count,
        embedding_model,
        fallback_reason,
        fallback_message,
    }
}

fn retrieval_state_from_engine(engine: &SearchEngine) -> RetrievalStateDto {
    let probe = embedding_runtime_availability_from_env();
    retrieval_state_from_parts(
        engine.semantic_doc_count(),
        engine
            .embedding_model_id()
            .map(str::to_string)
            .or(probe.model_id),
        engine.embedding_runtime_configured(),
        if engine.embedding_runtime_configured() {
            None
        } else {
            probe.fallback_message
        },
    )
}

fn retrieval_state_from_storage(storage: &Storage) -> Result<RetrievalStateDto, ApiError> {
    let LlmSymbolDocStats {
        doc_count,
        embedding_model,
    } = storage
        .get_llm_symbol_doc_stats()
        .map_err(|e| ApiError::internal(format!("Failed to query LLM symbol doc stats: {e}")))?;
    let probe = embedding_runtime_availability_from_env();
    Ok(retrieval_state_from_parts(
        doc_count,
        embedding_model.or(probe.model_id),
        probe.available,
        probe.fallback_message,
    ))
}

fn llm_indexable_kind(kind: codestory_contracts::graph::NodeKind) -> bool {
    !matches!(
        kind,
        codestory_contracts::graph::NodeKind::FILE
            | codestory_contracts::graph::NodeKind::UNKNOWN
            | codestory_contracts::graph::NodeKind::BUILTIN_TYPE
    )
}

fn edge_digest_for_node(
    storage: &Storage,
    node_id: codestory_contracts::graph::NodeId,
    limit: usize,
) -> Vec<String> {
    let mut by_kind = HashMap::<String, usize>::new();
    if let Ok(edges) = storage.get_edges_for_node_id(node_id) {
        for edge in edges {
            let key = format!("{:?}", edge.kind);
            *by_kind.entry(key).or_insert(0) += 1;
        }
    }

    let mut counts = by_kind.into_iter().collect::<Vec<_>>();
    counts.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    counts
        .into_iter()
        .take(limit)
        .map(|(kind, count)| format!("{kind}={count}"))
        .collect()
}

fn referenced_symbol_labels(
    storage: &Storage,
    node: &codestory_contracts::graph::Node,
    limit: usize,
) -> Vec<String> {
    let mut labels = Vec::new();
    let Ok(edges) = storage.get_edges_for_node_id(node.id) else {
        return labels;
    };

    for edge in edges {
        let (source, target) = edge.effective_endpoints();
        let other = if source == node.id {
            target
        } else if target == node.id {
            source
        } else {
            continue;
        };
        let Ok(Some(other_node)) = storage.get_node(other) else {
            continue;
        };
        if !llm_indexable_kind(other_node.kind) {
            continue;
        }
        let label = node_display_name(&other_node);
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

fn child_symbol_labels(
    storage: &Storage,
    node: &codestory_contracts::graph::Node,
    limit: usize,
) -> Vec<String> {
    let Ok(children) = storage.get_children_symbols(node.id) else {
        return Vec::new();
    };
    children
        .into_iter()
        .filter(|child| llm_indexable_kind(child.kind))
        .map(|child| node_display_name(&child))
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
    file_text_cache: &mut HashMap<String, Option<String>>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let Some(path) = file_path else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let contents = file_text_cache
        .entry(path.to_string())
        .or_insert_with(|| read_searchable_file_contents(path));
    let Some(contents) = contents.as_deref() else {
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
    storage: &Storage,
    node: &codestory_contracts::graph::Node,
    display_name: &str,
    file_path: Option<&str>,
    file_text_cache: &mut HashMap<String, Option<String>>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{LLM_SYMBOL_DOC_VERSION_PREFIX} {LLM_SYMBOL_DOC_SCHEMA_VERSION}"
    );
    let _ = writeln!(out, "symbol: {display_name}");
    let _ = writeln!(out, "kind: {:?}", node.kind);
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
    if let Some(line) = node.start_line {
        let _ = writeln!(out, "line: {line}");
    }
    if let Some(qualified_name) = node.qualified_name.as_deref() {
        let _ = writeln!(out, "qualified_name: {qualified_name}");
    }

    let (signature, comments, body) = symbol_excerpt(node, file_path, file_text_cache);
    if !signature.is_empty() {
        let _ = writeln!(out, "signature: {}", signature.join(" "));
    }
    if !comments.is_empty() {
        let _ = writeln!(out, "comments: {}", comments.join(" "));
    }
    if !body.is_empty() {
        let _ = writeln!(out, "body_summary: {}", body.join(" "));
    }

    let children = child_symbol_labels(storage, node, 6);
    if !children.is_empty() {
        let _ = writeln!(out, "members: {}", children.join(", "));
    }

    let related = referenced_symbol_labels(storage, node, 6);
    if !related.is_empty() {
        let _ = writeln!(out, "related_symbols: {}", related.join(", "));
    }

    let edge_digest = edge_digest_for_node(storage, node.id, 6);
    if !edge_digest.is_empty() {
        out.push_str("edge_digest:");
        for digest in edge_digest {
            let _ = write!(out, " {digest};");
        }
        out.push('\n');
    }

    out
}

fn map_llm_doc_to_search(doc: LlmSymbolDoc) -> LlmSearchDoc {
    LlmSearchDoc {
        node_id: doc.node_id,
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
}

fn llm_symbol_doc_hash(doc_text: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in LLM_SYMBOL_DOC_SCHEMA_VERSION
        .to_le_bytes()
        .into_iter()
        .chain(doc_text.as_bytes().iter().copied())
    {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

fn flush_pending_llm_symbol_docs(
    storage: &mut Storage,
    engine: &mut SearchEngine,
    pending: &mut Vec<PendingLlmSymbolDoc>,
    model_id: &str,
    updated_at_epoch_ms: i64,
    stats: &mut SemanticProjectionStats,
) -> Result<(), ApiError> {
    if pending.is_empty() {
        return Ok(());
    }

    let model_id = model_id.to_string();
    let batch = std::mem::take(pending);
    let payloads = batch
        .iter()
        .map(|doc| doc.doc_text.clone())
        .collect::<Vec<_>>();
    let embedding_started = Instant::now();
    let embeddings = engine
        .embed_texts(&payloads)
        .map_err(|e| ApiError::internal(format!("Failed to embed symbol docs: {e}")))?;
    stats.embedding_ms = stats
        .embedding_ms
        .saturating_add(clamp_u128_to_u32(embedding_started.elapsed().as_millis()));

    let docs = batch
        .into_iter()
        .zip(embeddings)
        .map(|(doc, embedding)| LlmSymbolDoc {
            node_id: doc.node_id,
            file_node_id: doc.file_node_id,
            kind: doc.kind,
            display_name: doc.display_name,
            qualified_name: doc.qualified_name,
            file_path: doc.file_path,
            start_line: doc.start_line,
            doc_text: doc.doc_text,
            doc_version: LLM_SYMBOL_DOC_SCHEMA_VERSION,
            doc_hash: doc.doc_hash,
            embedding_model: model_id.clone(),
            embedding_dim: embedding.len() as u32,
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

    if let Err(error) = engine.set_embedding_runtime_from_env() {
        tracing::warn!(
            "{EMBEDDING_MODEL_ENV} not configured or invalid ({error}); semantic ask retrieval will be unavailable until a local model artifact is configured. Use a bundled model at {DEFAULT_BUNDLED_EMBED_MODEL_PATH}, set {EMBEDDING_RUNTIME_MODE_ENV}=hash for local-dev embeddings, or set {HYBRID_RETRIEVAL_ENABLED_ENV}=false for lexical-only retrieval."
        );
        if hydrate_semantic_docs {
            let reload_started = Instant::now();
            reload_llm_docs_from_storage(storage, engine, LLM_DOC_RELOAD_BATCH_SIZE)?;
            stats.reload_ms = clamp_u128_to_u32(reload_started.elapsed().as_millis());
        }
        return Ok(stats);
    }

    let model_id = engine
        .embedding_model_id()
        .unwrap_or("sentence-transformers/all-MiniLM-L6-v2-local")
        .to_string();
    let updated_at_epoch_ms = current_epoch_ms();

    let existing_docs = storage
        .get_llm_symbol_doc_reuse_metadata()
        .map_err(|e| ApiError::internal(format!("Failed to load semantic doc metadata: {e}")))?
        .into_iter()
        .map(|doc| (doc.node_id, doc))
        .collect::<HashMap<_, _>>();

    if let Some(scope) = llm_refresh_file_scope {
        if scope.is_empty() {
            if hydrate_semantic_docs {
                let reload_started = Instant::now();
                reload_llm_docs_from_storage(storage, engine, LLM_DOC_RELOAD_BATCH_SIZE)?;
                stats.reload_ms = clamp_u128_to_u32(reload_started.elapsed().as_millis());
            }
            return Ok(stats);
        }
    }

    let embed_batch_size = llm_doc_embed_batch_size();
    tracing::debug!(embed_batch_size, "Using semantic doc embedding batch size");
    let mut pending_docs = Vec::<PendingLlmSymbolDoc>::with_capacity(embed_batch_size);
    let mut seen_node_ids = Vec::<codestory_contracts::graph::NodeId>::new();
    let mut file_text_cache = HashMap::<String, Option<String>>::new();

    for node in nodes {
        if !llm_indexable_kind(node.kind) {
            continue;
        }
        if let Some(scope) = llm_refresh_file_scope
            && !node
                .file_node_id
                .map(|file_node_id| scope.contains(&file_node_id))
                .unwrap_or(false)
        {
            continue;
        }
        let display_name = node_names
            .get(&node.id)
            .cloned()
            .unwrap_or_else(|| node_display_name(node));
        let file_path = AppController::file_path_for_node(storage, node)?;
        let doc_build_started = Instant::now();
        let doc_text = build_llm_symbol_doc_text(
            storage,
            node,
            &display_name,
            file_path.as_deref(),
            &mut file_text_cache,
        );
        stats.doc_build_ms = stats
            .doc_build_ms
            .saturating_add(clamp_u128_to_u32(doc_build_started.elapsed().as_millis()));
        let doc_hash = llm_symbol_doc_hash(&doc_text);
        seen_node_ids.push(node.id);
        if let Some(existing_doc) = existing_docs.get(&node.id)
            && existing_doc.doc_version == LLM_SYMBOL_DOC_SCHEMA_VERSION
            && existing_doc.doc_hash == doc_hash
            && existing_doc.embedding_model == model_id
            && existing_doc.embedding_dim > 0
        {
            stats.docs_reused = stats.docs_reused.saturating_add(1);
            continue;
        }
        stats.docs_pending = stats.docs_pending.saturating_add(1);
        pending_docs.push(PendingLlmSymbolDoc {
            node_id: node.id,
            file_node_id: node.file_node_id,
            kind: node.kind,
            display_name,
            qualified_name: node.qualified_name.clone(),
            file_path,
            start_line: node.start_line,
            doc_text,
            doc_hash,
        });
        if pending_docs.len() >= embed_batch_size {
            flush_pending_llm_symbol_docs(
                storage,
                engine,
                &mut pending_docs,
                &model_id,
                updated_at_epoch_ms,
                &mut stats,
            )?;
        }
    }

    flush_pending_llm_symbol_docs(
        storage,
        engine,
        &mut pending_docs,
        &model_id,
        updated_at_epoch_ms,
        &mut stats,
    )?;

    let stale_docs = if let Some(scope) = llm_refresh_file_scope {
        let file_node_ids = scope.iter().copied().collect::<Vec<_>>();
        storage
            .delete_llm_symbol_docs_for_files_except_node_ids(&file_node_ids, &seen_node_ids)
            .map_err(|e| ApiError::internal(format!("Failed to prune stale LLM docs: {e}")))?
    } else {
        storage
            .prune_llm_symbol_docs_to_node_ids(&seen_node_ids)
            .map_err(|e| ApiError::internal(format!("Failed to prune stale LLM docs: {e}")))?
    };
    stats.docs_stale = clamp_usize_to_u32(stale_docs);

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

fn lexical_hybrid_hits(
    engine: &mut SearchEngine,
    query: &str,
    graph_boosts: &HashMap<codestory_contracts::graph::NodeId, f32>,
) -> Vec<HybridSearchHit> {
    let lexical = engine.search_symbol_with_scores(query);
    let lexical_max = lexical
        .iter()
        .map(|(_, score)| *score)
        .fold(0.0_f32, f32::max)
        .max(1.0);
    lexical
        .into_iter()
        .map(|(node_id, score)| {
            let lexical_score = (score / lexical_max).clamp(0.0, 1.0);
            let graph_score = graph_boosts
                .get(&node_id)
                .copied()
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            HybridSearchHit {
                node_id,
                lexical_score,
                semantic_score: 0.0,
                graph_score,
                total_score: (0.85 * lexical_score + 0.15 * graph_score).clamp(0.0, 1.0),
            }
        })
        .collect::<Vec<_>>()
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

struct AppState {
    project_root: Option<PathBuf>,
    storage_path: Option<PathBuf>,
    node_names: HashMap<codestory_contracts::graph::NodeId, String>,
    search_engine: Option<SearchEngine>,
    is_indexing: bool,
}

/// GUI-agnostic orchestrator for CodeStory.
///
/// This is intentionally "headless": any app shell (CLI, desktop, IDE integration)
/// should call methods on this controller and subscribe to `AppEventPayload`.
#[derive(Clone)]
pub struct AppController {
    state: Arc<Mutex<AppState>>,
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
            })),
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

    fn run_local_agent(
        &self,
        connection: &AgentConnectionSettingsDto,
        prompt: &str,
    ) -> Result<LocalAgentResponse, ApiError> {
        agent::local_runner::run_local_agent(self, connection, prompt)
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
        s.search_engine = None;
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
            s.search_engine = Some(engine);
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
            score,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            resolvable: true,
        })
    }

    fn repo_text_enabled_for_mode(mode: SearchRepoTextMode, query: &str) -> bool {
        match mode {
            SearchRepoTextMode::Auto => looks_like_repo_text_query(query),
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
    ) -> Result<Vec<SearchHit>, ApiError> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let mut hits = Vec::new();
        let mut seen = indexed_hit_ids.clone();
        let terms = extract_symbol_search_terms(query);
        let normalized_query = query.trim().to_ascii_lowercase();
        for file in storage
            .get_files()
            .map_err(|e| ApiError::internal(format!("Failed to load files for text search: {e}")))?
        {
            if hits.len() >= limit {
                break;
            }

            let path_string = file.path.to_string_lossy().to_string();
            let Some(contents) = read_searchable_file_contents(&path_string) else {
                continue;
            };
            if contents.len() > 1_000_000 {
                continue;
            }
            let Some(line) = file_text_match_line(&contents, query, &terms) else {
                continue;
            };
            let node_id = NodeId::from(codestory_contracts::graph::NodeId(file.id));
            if !seen.insert(node_id.clone()) {
                continue;
            }

            let display_name = project_root
                .and_then(|root| file.path.strip_prefix(root).ok())
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .or_else(|| {
                    file.path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                })
                .unwrap_or_else(|| path_string.clone());
            let exact_match = !normalized_query.is_empty()
                && contents.to_ascii_lowercase().contains(&normalized_query);
            let score = if exact_match { 260.0 } else { 150.0 } - hits.len() as f32;
            hits.push(SearchHit {
                node_id,
                display_name,
                kind: codestory_contracts::api::NodeKind::FILE,
                file_path: Some(path_string),
                line: Some(line),
                score,
                origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
                resolvable: false,
            });
        }

        hits.sort_by(|left, right| compare_search_hits(query, left, right));
        hits.truncate(limit);
        Ok(hits)
    }

    fn lexical_symbol_hits(
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
        hits.sort_by(|left, right| compare_search_hits(query, left, right));
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
        };

        Ok(ProjectSummary {
            root: root.to_string_lossy().to_string(),
            stats: dto_stats,
            retrieval: Some(retrieval_state_from_storage(storage)?),
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
            s.search_engine = None;
        }

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
            s.search_engine = Some(engine);
        }

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
            Ok(mut summary) => {
                let cache_refresh_started = Instant::now();
                let semantic_stats = if refresh_runtime_caches {
                    let mut storage = Storage::open(&storage_path).map_err(|e| {
                        ApiError::internal(format!("Failed to reopen storage: {e}"))
                    })?;
                    refresh_caches(
                        self,
                        &mut storage,
                        &storage_path,
                        summary.llm_refresh_scope.as_ref(),
                    )?
                } else {
                    self.finalize_indexing_without_runtime_refresh_with(
                        &storage_path,
                        summary.llm_refresh_scope.as_ref(),
                        |storage, llm_refresh_scope| {
                            rebuild_search_state_from_storage(
                                storage,
                                &storage_path,
                                llm_refresh_scope,
                                false,
                            )
                            .map(|result| result.semantic_stats)
                        },
                    )?
                };
                summary.phase_timings.cache_refresh_ms = Some(clamp_u128_to_u32(
                    cache_refresh_started.elapsed().as_millis(),
                ));
                apply_semantic_projection_stats(&mut summary.phase_timings, semantic_stats);
                Ok(summary.phase_timings)
            }
            Err(error) => {
                if refresh_runtime_caches {
                    if let Ok(mut storage) = Storage::open(&storage_path) {
                        let _ = refresh_caches(self, &mut storage, &storage_path, None);
                    } else {
                        self.clear_search_state();
                        self.state.lock().is_indexing = false;
                    }
                } else {
                    self.clear_search_state();
                    self.state.lock().is_indexing = false;
                }
                Err(error)
            }
        }
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

    fn finalize_indexing_without_runtime_refresh_with<F>(
        &self,
        storage_path: &Path,
        llm_refresh_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
        rebuild: F,
    ) -> Result<SemanticProjectionStats, ApiError>
    where
        F: FnOnce(
            &mut Storage,
            Option<&HashSet<codestory_contracts::graph::NodeId>>,
        ) -> Result<SemanticProjectionStats, ApiError>,
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

    pub fn search(&self, req: SearchRequest) -> Result<Vec<SearchHit>, ApiError> {
        Ok(self.search_results(req)?.hits)
    }

    pub fn search_results(&self, req: SearchRequest) -> Result<SearchResultsDto, ApiError> {
        self.ensure_consistent_read_state("Search")?;
        let query = req.query.clone();
        let limit_per_source = req.limit_per_source.clamp(1, 50) as usize;
        let repo_text_mode = req.repo_text;
        let (mut indexed_symbol_hits, retrieval) = self.search_hybrid_results(
            SearchRequest {
                query: query.clone(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: req.limit_per_source,
            },
            None,
            limit_per_source,
            None,
        )?;
        let storage = self.open_storage()?;

        if should_expand_symbol_query(&query, indexed_symbol_hits.len()) {
            let expanded = {
                let mut s = self.state.lock();
                let engine = s.search_engine.as_mut().ok_or_else(|| {
                    ApiError::invalid_argument(
                        "Search engine not initialized. Open a project first.",
                    )
                })?;
                let direct_matches = engine.search_symbol_with_scores(&query);
                let terms = extract_symbol_search_terms(&query);
                if terms.is_empty() {
                    None
                } else {
                    let mut expanded = Vec::<(codestory_contracts::graph::NodeId, f32)>::new();
                    for term in terms {
                        for (id, score) in engine.search_symbol_with_scores(&term) {
                            expanded.push((id, score));
                        }
                        if let Ok(ids) = engine.search_full_text(&term) {
                            for (rank, id) in ids.into_iter().enumerate() {
                                let text_score = 40.0_f32 - (rank as f32 * 1.5);
                                expanded.push((id, text_score));
                            }
                        }
                    }
                    Some((
                        aggregate_symbol_matches(direct_matches, expanded),
                        s.node_names.clone(),
                    ))
                }
            };
            if let Some((expanded_matches, node_names)) = expanded {
                let expanded_hits = expanded_matches
                    .into_iter()
                    .filter_map(|(id, score)| {
                        Self::build_search_hit(&storage, &node_names, id, score)
                    })
                    .collect::<Vec<_>>();
                merge_search_hits_by_node_id(&mut indexed_symbol_hits, expanded_hits);
            }
        }

        indexed_symbol_hits.sort_by(|left, right| compare_search_hits(&query, left, right));
        indexed_symbol_hits.truncate(limit_per_source);

        let repo_text_enabled = Self::repo_text_enabled_for_mode(repo_text_mode, &query);
        let indexed_hit_ids = indexed_symbol_hits
            .iter()
            .map(|hit| hit.node_id.clone())
            .collect::<HashSet<_>>();
        let repo_text_hits = if repo_text_enabled {
            Self::collect_repo_text_hits(
                &storage,
                self.require_project_root().ok().as_deref(),
                &query,
                limit_per_source,
                &indexed_hit_ids,
            )?
        } else {
            Vec::new()
        };
        let mut hits = Vec::with_capacity(indexed_symbol_hits.len() + repo_text_hits.len());
        hits.extend(indexed_symbol_hits.iter().cloned());
        hits.extend(repo_text_hits.iter().cloned());

        Ok(SearchResultsDto {
            query,
            retrieval,
            limit_per_source: limit_per_source as u32,
            repo_text_mode,
            repo_text_enabled,
            indexed_symbol_hits,
            repo_text_hits,
            hits,
        })
    }

    fn search_hybrid_results(
        &self,
        req: SearchRequest,
        focus_node_id: Option<NodeId>,
        max_results: usize,
        request_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<(Vec<SearchHit>, RetrievalStateDto), ApiError> {
        let (scored_hits, retrieval) =
            self.search_hybrid_scored_inner(req, focus_node_id, max_results, request_weights)?;
        Ok((
            scored_hits.into_iter().map(|scored| scored.hit).collect(),
            retrieval,
        ))
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
        Ok(self
            .search_hybrid_scored_inner(req, focus_node_id, max_results, request_weights)?
            .0)
    }

    fn search_hybrid_scored_inner(
        &self,
        req: SearchRequest,
        focus_node_id: Option<NodeId>,
        max_results: usize,
        request_weights: Option<AgentHybridWeightsDto>,
    ) -> Result<(Vec<HybridSearchScoredHit>, RetrievalStateDto), ApiError> {
        self.ensure_search_state()?;
        let storage = self.open_storage()?;
        let storage_retrieval = retrieval_state_from_storage(&storage)?;
        let mut graph_boosts = HashMap::<codestory_contracts::graph::NodeId, f32>::new();

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
            let mut retrieval = storage_retrieval.clone();

            if retrieval.mode == RetrievalModeDto::Hybrid && engine.semantic_doc_count() == 0 {
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
                retrieval = retrieval_state_from_engine(engine);
            } else if engine.semantic_doc_count() > 0 || engine.embedding_runtime_configured() {
                retrieval = retrieval_state_from_engine(engine);
            }

            let hits = if retrieval.mode == RetrievalModeDto::Hybrid {
                let mut config = HybridSearchConfig {
                    max_results: max_results.clamp(1, 50),
                    ..HybridSearchConfig::default()
                };
                let (lexical_weight, semantic_weight, graph_weight) =
                    normalized_hybrid_weights(request_weights, &config);
                config.lexical_weight = lexical_weight;
                config.semantic_weight = semantic_weight;
                config.graph_weight = graph_weight;

                match engine.search_hybrid_with_scores(&req.query, &graph_boosts, config) {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::warn!(
                            "Hybrid retrieval failed for query {:?}; falling back to symbolic ranking: {}",
                            req.query,
                            error
                        );
                        retrieval = retrieval_state_from_parts(
                            engine.semantic_doc_count(),
                            engine.embedding_model_id().map(str::to_string),
                            false,
                            Some(format!(
                                "Semantic query fallback engaged after runtime error: {error}"
                            )),
                        );
                        lexical_hybrid_hits(engine, &req.query, &graph_boosts)
                    }
                }
            } else {
                lexical_hybrid_hits(engine, &req.query, &graph_boosts)
            };

            (hits, s.node_names.clone(), retrieval)
        };

        let mut out = Vec::with_capacity(hybrid.len());
        for scored in hybrid {
            if let Some(hit) =
                Self::build_search_hit(&storage, &node_names, scored.node_id, scored.total_score)
            {
                out.push(HybridSearchScoredHit {
                    hit,
                    lexical_score: scored.lexical_score,
                    semantic_score: scored.semantic_score,
                    graph_score: scored.graph_score,
                    total_score: scored.total_score,
                });
            }
        }
        out.sort_by(|left, right| compare_search_hits(&req.query, &left.hit, &right.hit));
        out.truncate(max_results.clamp(1, 50));

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

    pub fn graph_neighborhood(&self, req: GraphRequest) -> Result<GraphResponse, ApiError> {
        graph_builders::graph_neighborhood(self, req)
    }

    pub fn graph_trail(&self, req: TrailConfigDto) -> Result<GraphResponse, ApiError> {
        graph_builders::graph_trail(self, req)
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
            semantic_doc_build_ms: None,
            semantic_embedding_ms: None,
            semantic_db_upsert_ms: None,
            semantic_reload_ms: None,
            semantic_docs_reused: None,
            semantic_docs_embedded: None,
            semantic_docs_pending: None,
            semantic_docs_stale: None,
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
            semantic_doc_build_ms: None,
            semantic_embedding_ms: None,
            semantic_db_upsert_ms: None,
            semantic_reload_ms: None,
            semantic_docs_reused: None,
            semantic_docs_embedded: None,
            semantic_docs_pending: None,
            semantic_docs_stale: None,
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

    Ok(RefreshInputs {
        stored_files: stored_files.collect(),
        inventory: WorkspaceInventory::from_records(inventory),
    })
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
) -> Result<SemanticProjectionStats, ApiError> {
    let refreshed =
        rebuild_search_state_from_storage(storage, storage_path, llm_refresh_scope, true);

    let mut s = controller.state.lock();
    match refreshed {
        Ok(result) => {
            let semantic_stats = result.semantic_stats;
            s.node_names = result.node_names;
            s.search_engine = Some(result.engine);
            s.is_indexing = false;
            Ok(semantic_stats)
        }
        Err(error) => {
            tracing::warn!(
                "Failed to rebuild search caches from storage: {}",
                error.message
            );
            s.node_names.clear();
            s.search_engine = None;
            s.is_indexing = false;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{
        Edge, EdgeId, EdgeKind, Node, NodeId as CoreNodeId, NodeKind, Occurrence, OccurrenceKind,
        SourceLocation,
    };
    use crossbeam_channel::unbounded;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

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

    fn hybrid_test_env() -> Vec<EnvGuard> {
        vec![
            EnvGuard::set(HYBRID_RETRIEVAL_ENABLED_ENV, "true"),
            EnvGuard::set(EMBEDDING_RUNTIME_MODE_ENV, "hash"),
            EnvGuard::remove(EMBEDDING_MODEL_ENV),
            EnvGuard::remove(EMBEDDING_TOKENIZER_ENV),
        ]
    }

    #[test]
    fn llm_doc_embed_batch_size_uses_throughput_default() {
        assert_eq!(llm_doc_embed_batch_size(), 64);
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
        fs::write(
            src.join("lib.rs"),
            format!(
                r#"
/// {digest_text}
pub fn build_snapshot_digest() -> &'static str {{
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
    fn search_ranks_exact_type_before_members_and_omits_unknown_hits() {
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

        let hits = controller
            .search(SearchRequest {
                query: "AppController".to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 10,
            })
            .expect("search");

        assert_eq!(
            hits.first().map(|hit| hit.display_name.as_str()),
            Some("AppController")
        );
        assert!(
            hits.iter()
                .all(|hit| hit.kind != codestory_contracts::api::NodeKind::UNKNOWN)
        );
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
            resolvable: true,
        };
        let method = SearchHit {
            node_id: NodeId("method".to_string()),
            display_name: "ArtificialPlayer::min_max".to_string(),
            kind: codestory_contracts::api::NodeKind::METHOD,
            file_path: None,
            line: None,
            score: 184.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            resolvable: true,
        };

        let mut hits = [method, function.clone()];
        hits.sort_by(|left, right| compare_search_hits("min_max", left, right));

        assert_eq!(hits.first().map(|hit| hit.kind), Some(function.kind));
    }

    #[test]
    fn search_prefers_real_tictactoe_definitions_for_check_winner_and_min_max() {
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

        for (query, expected_kind) in [
            ("check_winner", codestory_contracts::api::NodeKind::FUNCTION),
            ("min_max", codestory_contracts::api::NodeKind::FUNCTION),
        ] {
            let hits = controller
                .search(SearchRequest {
                    query: query.to_string(),
                    repo_text: SearchRepoTextMode::Off,
                    limit_per_source: 10,
                })
                .expect("search fixtures");
            let first = hits.first().expect("at least one hit");
            assert_eq!(
                first.kind, expected_kind,
                "expected real definition to outrank loose matches for {query}"
            );
            assert_eq!(terminal_symbol_segment(&first.display_name), query);
        }
    }

    #[test]
    fn search_expands_natural_language_queries() {
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
                ])
                .expect("insert nodes");
        }

        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let hits = controller
            .search(SearchRequest {
                query: "How does the language parsing work in this repo?".to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 20,
            })
            .expect("search with natural language");

        assert!(
            !hits.is_empty(),
            "Expected term extraction fallback to find symbol matches"
        );
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
        let mut engine = result.engine;
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
            incremental_timings.semantic_docs_embedded.unwrap_or(0) > 0,
            "new semantic docs from the touched file should be embedded"
        );
        assert!(
            incremental_timings
                .semantic_docs_embedded
                .unwrap_or(u32::MAX)
                < clamp_usize_to_u32(before_docs.len()),
            "incremental semantic sync should not re-embed untouched files"
        );
        assert_eq!(
            incremental_timings.semantic_docs_stale.unwrap_or(0),
            0,
            "adding a symbol should not make existing semantic docs stale"
        );

        let docs = Storage::open(&storage_path)
            .expect("reopen storage")
            .get_all_llm_symbol_docs()
            .expect("semantic docs after incremental");
        assert!(
            docs.iter()
                .any(|doc| doc.display_name.contains("codestory_added_move_hint")),
            "incremental semantic docs should include the new symbol"
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
    fn search_results_group_runtime_owned_repo_text_hits() {
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

        let results = controller
            .search_results(SearchRequest {
                query: "how does alpha work".to_string(),
                repo_text: SearchRepoTextMode::On,
                limit_per_source: 5,
            })
            .expect("search results");

        assert!(results.repo_text_enabled);
        assert!(!results.indexed_symbol_hits.is_empty());
        assert!(!results.repo_text_hits.is_empty());
        assert_eq!(
            results.hits.len(),
            results.indexed_symbol_hits.len() + results.repo_text_hits.len()
        );
        assert_eq!(
            results.repo_text_hits[0].origin,
            codestory_contracts::api::SearchHitOrigin::TextMatch
        );
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
            .find(|node| node.kind == NodeKind::FUNCTION)
            .map(|node| NodeId::from(node.id))
            .expect("function node");
        drop(storage);

        let context = controller
            .symbol_context(symbol_id)
            .expect("symbol context by id");
        assert!(
            !context.related_hits.is_empty(),
            "expected lexical related hits for symbol context"
        );

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

        storage
            .get_connection()
            .execute(
                "UPDATE llm_symbol_doc
                 SET embedding_model = CASE
                     WHEN node_id = 2 THEN 'model-b'
                     ELSE embedding_model
                 END",
                [],
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
                resolvable: true,
            },
            SearchHit {
                node_id: NodeId("secondary".to_string()),
                display_name: "alpha".to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                file_path: Some("src/lib.rs".to_string()),
                line: Some(20),
                score: 0.75,
                origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                resolvable: true,
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
                resolvable: true,
            }],
        );

        hits.sort_by(|left, right| compare_search_hits("alpha", left, right));

        assert_eq!(hits[0].node_id, NodeId("primary".to_string()));
        assert_eq!(hits[0].score, 250.0);
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
        let initial_doc = storage
            .get_all_llm_symbol_docs()
            .expect("load initial semantic docs")
            .into_iter()
            .find(|doc| doc.display_name == "build_snapshot_digest")
            .expect("initial digest doc");
        assert!(initial_doc.doc_text.contains("initial compressed digest"));
        drop(storage);

        write_reindex_semantic_fixture(workspace.path(), "updated compressed digest");
        controller
            .run_indexing_blocking(IndexMode::Full)
            .expect("rerun full index");

        let storage = Storage::open(&storage_path).expect("open storage after rerun");
        let updated_doc = storage
            .get_all_llm_symbol_docs()
            .expect("load updated semantic docs")
            .into_iter()
            .find(|doc| doc.display_name == "build_snapshot_digest")
            .expect("updated digest doc");
        assert!(updated_doc.doc_text.contains("updated compressed digest"));
        assert!(
            !updated_doc.doc_text.contains("initial compressed digest"),
            "full index should rebuild semantic docs instead of reusing stale persisted content"
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
            state.search_engine = Some(SearchEngine::new(None).expect("search engine"));
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
            })
            .expect_err("search should be blocked while indexing");

        assert_eq!(error.code, "invalid_argument");
        assert!(error.message.contains("indexing is in progress"));
    }

    #[test]
    fn search_lazily_initializes_search_state_after_summary_open() {
        let workspace = copy_tictactoe_workspace();
        let storage_path = workspace.path().join(".cache").join("codestory.db");
        let controller = AppController::new();

        controller
            .open_project_summary_with_storage_path(workspace.path().to_path_buf(), storage_path)
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("index without runtime refresh");

        let hits = controller
            .search(SearchRequest {
                query: "check_winner".to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 10,
            })
            .expect("lazy search should succeed");

        assert!(
            !hits.is_empty(),
            "expected search to return indexed symbols"
        );
        let state = controller.state.lock();
        assert!(state.search_engine.is_some());
        assert!(!state.node_names.is_empty());
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
