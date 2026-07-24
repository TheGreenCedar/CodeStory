use super::{
    AccessKind, ApiError, BTreeMap, BTreeSet, BUILD_EDGE_SEED_BATCH_SIZE, CancellationToken,
    DenseAnchorInput, DenseAnchorInputReuseMetadata, EmbeddingProfileContractDto, FileInfo,
    GraphEdge, GraphNode, GraphNodeId, HashMap, HashSet, IndexPublicationRecord,
    IndexingPhaseTimings, Instant, LlmSymbolDoc, LlmSymbolDocStats, Path, RetrievalFileRole,
    SEMANTIC_FILE_TEXT_CACHE_MAX_BYTES, SEMANTIC_FILE_TEXT_MAX_BYTES, SearchEngine,
    SourceIndexPolicy, SourcePolicyExclusionPolicyIdentity, Storage, StoreFileRole,
    StoredSemanticDocsContractDto, SymbolSearchDoc, clamp_u128_to_u32, clamp_usize_to_u32,
    current_epoch_ms, indexing_cancelled_error, is_indexing_cancelled, node_display_name,
    read_file_text_limited, retrieval_file_role_from_path, semantic_doc_language_from_path,
    semantic_path_aliases, semantic_symbol_aliases, semantic_symbol_role_aliases,
};
#[cfg(test)]
use super::{embedding_profile_contract_from_env, test_sidecar_runtime_from_env};
#[cfg(test)]
use crate::publication::{PublicationTestBoundary, publication_test_checkpoint};
#[cfg(test)]
use crate::search;
use crate::search_state::reload_llm_docs_from_storage;
use rayon::prelude::*;
use serde::Serialize;
use std::fmt::Write as _;

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
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn stored_semantic_embeddings_for_test(storage_path: &Path) -> anyhow::Result<Vec<Vec<f32>>> {
    Ok(Store::open_read_only(storage_path)?
        .get_all_llm_symbol_docs()?
        .into_iter()
        .map(|document| document.embedding)
        .collect())
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SemanticProjectionStats {
    pub(super) reported: bool,
    pub(super) semantic_context_index_ms: u32,
    pub(super) node_load_ms: u32,
    pub(super) node_load_rows: u32,
    pub(super) node_stream_batches: u32,
    pub(super) endpoint_load_ms: u32,
    pub(super) endpoint_load_rows: u32,
    pub(super) endpoint_load_batches: u32,
    pub(super) selected_nodes: u32,
    pub(super) context_file_count: u32,
    pub(super) context_path_bytes: u32,
    pub(super) node_lookup_entries: u32,
    pub(super) context_ms: u32,
    pub(super) doc_build_ms: u32,
    pub(super) embedding_ms: u32,
    pub(super) db_upsert_ms: u32,
    pub(super) reload_ms: u32,
    pub(super) prune_ms: u32,
    pub(super) docs_reused: u32,
    pub(super) docs_embedded: u32,
    pub(super) docs_pending: u32,
    pub(super) docs_stale: u32,
    pub(super) symbol_search_docs_written: u32,
    pub(super) dense_docs_skipped: u32,
    pub(super) dense_public_api: u32,
    pub(super) dense_entrypoint: u32,
    pub(super) dense_documented_nontrivial: u32,
    pub(super) dense_central_graph_node: u32,
    pub(super) dense_component_report: u32,
    pub(super) dense_unstructured_doc: u32,
}

pub(super) struct ComponentReportRefreshScope {
    pub(super) previous_file_paths: HashMap<codestory_contracts::graph::NodeId, String>,
    pub(super) removed_component_keys: HashSet<String>,
}

#[derive(Clone, Copy)]
pub(super) struct SemanticRefreshScope<'a> {
    file_ids: Option<&'a HashSet<codestory_contracts::graph::NodeId>>,
    component_reports: Option<&'a ComponentReportRefreshScope>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SearchStateBuildStats {
    pub(super) search_projection_rebuild_ms: u32,
    pub(super) search_symbol_stream_ms: u32,
    pub(super) search_symbol_stream_rows: u32,
    pub(super) search_symbol_stream_batches: u32,
    pub(super) search_symbol_index_ms: u32,
    pub(super) search_symbol_index_docs_written: u32,
    pub(super) search_symbol_index_writer_count: u32,
    pub(super) search_symbol_index_commit_count: u32,
    pub(super) search_symbol_index_reload_count: u32,
    pub(super) search_symbol_index_commit_ms: u32,
    pub(super) search_symbol_index_reload_ms: u32,
}

pub(super) struct SearchStateBuildResult {
    pub(super) publication: Option<IndexPublicationRecord>,
    pub(super) node_names: HashMap<codestory_contracts::graph::NodeId, String>,
    pub(super) engine: SearchEngine,
    pub(super) search_stats: SearchStateBuildStats,
    pub(super) semantic_stats: SemanticProjectionStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct CacheRefreshStats {
    pub(super) search_stats: SearchStateBuildStats,
    pub(super) semantic_stats: SemanticProjectionStats,
    pub(super) runtime_cache_publish_ms: Option<u32>,
}

pub(super) fn apply_semantic_projection_stats(
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

pub(super) fn apply_cache_refresh_stats(
    timings: &mut IndexingPhaseTimings,
    stats: CacheRefreshStats,
) {
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
pub(super) fn build_search_state(
    search_storage_path: Option<&Path>,
    nodes: Vec<codestory_contracts::graph::Node>,
) -> Result<SearchStateBuildResult, ApiError> {
    build_search_state_for_nodes(search_storage_path, nodes, None)
}

#[cfg(test)]
pub(super) fn build_search_state_for_nodes(
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

pub(super) fn summarize_symbol_doc(
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

pub(super) fn summary_endpoint_http_error(
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

pub(super) fn local_symbol_summary(doc: &LlmSymbolDoc) -> String {
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

pub(super) const LLM_SYMBOL_DOC_SCHEMA_VERSION: u32 = 6;
pub(super) const LLM_SYMBOL_DOC_VERSION_PREFIX: &str = "semantic_doc_version:";
#[cfg(test)]
pub(super) const SEARCH_NODE_BATCH_SIZE: usize = 8_192;
pub(super) const SEARCH_SYMBOL_STREAM_BATCH_SIZE: usize = 4_096;
pub(super) const SEMANTIC_NODE_STREAM_BATCH_SIZE: usize = 4_096;
pub(super) const SEMANTIC_EDGE_STREAM_BATCH_SIZE: usize = 4_096;
pub(super) const LLM_DOC_RELOAD_BATCH_SIZE: usize = 512;
#[cfg(test)]
pub(super) const LLM_DOC_EMBED_BATCH_SIZE: usize = 128;
#[cfg(test)]
pub(super) const LLM_DOC_EMBED_BATCH_SIZE_ENV: &str = "CODESTORY_LLM_DOC_EMBED_BATCH_SIZE";
#[cfg(test)]
pub(super) const SEMANTIC_DOC_SCOPE_ENV: &str = "CODESTORY_SEMANTIC_DOC_SCOPE";
#[cfg(test)]
pub(super) const SEMANTIC_DOC_ALIAS_MODE_ENV: &str = "CODESTORY_SEMANTIC_DOC_ALIAS_MODE";
#[cfg(test)]
pub(super) const SEMANTIC_DOC_MAX_TOKENS_ENV: &str = "CODESTORY_SEMANTIC_DOC_MAX_TOKENS";
#[cfg(test)]
pub(super) const SEMANTIC_DOC_DEFAULT_MAX_TOKENS: usize = 128;
#[cfg(test)]
pub(super) const SEMANTIC_STREAM_PENDING_DOCS_ENV: &str = "CODESTORY_SEMANTIC_STREAM_PENDING_DOCS";
#[cfg(test)]
pub(super) const SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV: &str =
    "CODESTORY_SEMANTIC_STREAM_SORT_WINDOW_BATCHES";
#[cfg(test)]
pub(super) const SEMANTIC_STREAM_SORT_WINDOW_BATCHES: usize = 1;
pub(super) const SEMANTIC_POLICY_VERSION: &str = codestory_retrieval::SEMANTIC_POLICY_VERSION;
pub(super) const LEGACY_SEMANTIC_PROJECTION_SCHEMA_VERSION: u32 = 29;
pub(super) const LEGACY_OVERSIZED_SOURCE_POLICY_VERSION: &str = "oversized-source-v1";
pub(super) const SYMBOL_SEARCH_DOC_PROVENANCE: &str = "extracted";
pub(super) const DENSE_CENTRAL_RELATIONSHIP_THRESHOLD: usize = 12;
pub(super) const DENSE_CENTRAL_SCORE_THRESHOLD: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DenseAnchorReason {
    PublicApi,
    Entrypoint,
    DocumentedNontrivial,
    CentralGraphNode,
    ComponentReport,
    UnstructuredDoc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SemanticProjectionSourcePolicyCompatibility {
    Exact,
    LegacyPredecessor,
}

pub(super) fn semantic_projection_source_policy_compatibility(
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
    pub(super) fn as_str(self) -> &'static str {
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
pub(super) enum SemanticDocScope {
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
pub(super) enum SemanticDocAliasMode {
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
pub(super) fn semantic_doc_shape_contract() -> String {
    let max_tokens = semantic_doc_max_tokens_from_env();
    format!(
        "semantic_doc_version={};scope={};alias_mode={};max_tokens={}",
        LLM_SYMBOL_DOC_SCHEMA_VERSION,
        semantic_doc_scope_from_env().as_str(),
        semantic_doc_alias_mode_from_env().as_str(),
        max_tokens
    )
}

pub(super) fn semantic_doc_shape_contract_for_runtime(
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
pub(super) fn current_embedding_contract_from_env() -> Option<EmbeddingProfileContractDto> {
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

pub(super) fn current_embedding_contract_for_runtime(
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

pub(super) fn semantic_doc_stats_match_contract(
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

pub(super) fn stored_semantic_docs_contract_from_stats(
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
pub(super) fn semantic_doc_scope_from_env() -> SemanticDocScope {
    semantic_doc_scope_from_value(&std::env::var(SEMANTIC_DOC_SCOPE_ENV).unwrap_or_default())
}

pub(super) fn semantic_doc_scope_from_value(value: &str) -> SemanticDocScope {
    match value.trim().to_ascii_lowercase().as_str() {
        "all" | "full" | "all-symbols" | "all_symbols" => SemanticDocScope::AllSymbols,
        _ => SemanticDocScope::DurableSymbols,
    }
}

#[cfg(test)]
pub(super) fn semantic_doc_alias_mode_from_env() -> SemanticDocAliasMode {
    semantic_doc_alias_mode_from_value(
        &std::env::var(SEMANTIC_DOC_ALIAS_MODE_ENV).unwrap_or_default(),
    )
}

pub(super) fn semantic_doc_alias_mode_from_value(value: &str) -> SemanticDocAliasMode {
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
pub(super) fn semantic_doc_max_tokens_from_env() -> usize {
    std::env::var(SEMANTIC_DOC_MAX_TOKENS_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.clamp(16, 8_192))
        .unwrap_or(SEMANTIC_DOC_DEFAULT_MAX_TOKENS)
}

#[cfg(test)]
pub(super) fn stream_pending_llm_symbol_docs_from_env() -> bool {
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
pub(super) fn semantic_stream_sort_window_batches_from_env() -> usize {
    std::env::var(SEMANTIC_STREAM_SORT_WINDOW_BATCHES_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1, 16))
        .unwrap_or(SEMANTIC_STREAM_SORT_WINDOW_BATCHES)
}

pub(super) fn llm_indexable_kind_for_scope(
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

pub(super) fn llm_indexable_kinds_for_scope(
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
pub(super) fn llm_indexable_kind(kind: codestory_contracts::graph::NodeKind) -> bool {
    llm_indexable_kind_for_scope(kind, semantic_doc_scope_from_env())
}

pub(super) fn normalize_semantic_store_path(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = path.strip_prefix("//?/UNC/") {
        return format!("//{rest}");
    }
    if let Some(rest) = path.strip_prefix("//?/") {
        return rest.to_string();
    }
    path
}

pub(super) fn semantic_path_is_absolute_like(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || (bytes.len() > 2
            && bytes[1] == b':'
            && bytes[2] == b'/'
            && bytes[0].is_ascii_alphabetic())
}

pub(super) fn semantic_path_parent(path: &str) -> Option<&str> {
    path.rsplit_once('/')
        .map(|(parent, _)| parent)
        .filter(|parent| !parent.is_empty())
}

pub(super) fn common_semantic_path_prefix(left: &str, right: &str) -> String {
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

pub(super) fn common_absolute_semantic_parent(paths: &[(GraphNodeId, String)]) -> Option<String> {
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

pub(super) fn strip_semantic_common_parent(path: &str, common_parent: &str) -> Option<String> {
    let rest = path.strip_prefix(common_parent)?;
    let rest = rest.strip_prefix('/')?;
    (!rest.is_empty()).then(|| rest.to_string())
}

pub(super) fn semantic_file_table_path_maps(
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

pub(super) fn semantic_file_table_path_map(files: Vec<FileInfo>) -> HashMap<GraphNodeId, String> {
    let (display_paths, _) = semantic_file_table_path_maps(files);
    display_paths
}

#[derive(Default)]
pub(super) struct SemanticDocGraphContext {
    pub(super) child_labels: HashMap<GraphNodeId, Vec<String>>,
    pub(super) referenced_labels: HashMap<GraphNodeId, Vec<String>>,
    pub(super) edge_digests: HashMap<GraphNodeId, Vec<String>>,
    pub(super) centrality: HashMap<GraphNodeId, DenseAnchorCentrality>,
    pub(super) file_paths: HashMap<GraphNodeId, String>,
    pub(super) file_read_paths: HashMap<GraphNodeId, String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SemanticDocGraphPageStats {
    pub(super) endpoint_load_ms: u32,
    pub(super) endpoint_rows: u32,
    pub(super) endpoint_query_batches: u32,
    pub(super) lookup_entries: u32,
}

#[derive(Debug, Default)]
pub(super) struct SemanticNodeGraphSummary {
    child_labels: Vec<String>,
    referenced_labels: Vec<String>,
    edge_kind_counts: HashMap<String, usize>,
    centrality: DenseAnchorCentrality,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct DenseAnchorCentrality {
    pub(super) child_count: usize,
    pub(super) related_count: usize,
    pub(super) edge_count: usize,
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

pub(super) fn semantic_graph_node<'a>(
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
    pub(super) fn build(
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

    pub(super) fn build_for_scope(
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

    pub(super) fn build_for_full_page(
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

    pub(super) fn file_path_for_node(&self, node: &GraphNode) -> Option<&str> {
        node.file_node_id
            .and_then(|file_id| self.file_paths.get(&file_id))
            .map(String::as_str)
    }

    pub(super) fn file_read_path_for_node(&self, node: &GraphNode) -> Option<&str> {
        node.file_node_id.and_then(|file_id| {
            self.file_read_paths
                .get(&file_id)
                .or_else(|| self.file_paths.get(&file_id))
                .map(String::as_str)
        })
    }
}

pub(super) fn semantic_graph_dependent_file_ids_by_seed(
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

pub(super) fn build_semantic_file_text_cache(
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

pub(super) fn build_semantic_file_text_cache_with_limits(
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

pub(super) fn build_semantic_file_text_cache_from_paths(
    file_paths: &HashMap<String, String>,
) -> HashMap<String, Option<String>> {
    build_semantic_file_text_cache_from_paths_with_limits(
        file_paths,
        SEMANTIC_FILE_TEXT_MAX_BYTES,
        SEMANTIC_FILE_TEXT_CACHE_MAX_BYTES,
    )
}

pub(super) fn build_semantic_file_text_cache_from_paths_with_limits(
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

pub(super) fn edge_digest_for_edges(edges: &[GraphEdge], limit: usize) -> Vec<String> {
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

pub(super) fn edge_digest_for_node(
    storage: &Storage,
    node_id: GraphNodeId,
    limit: usize,
) -> Vec<String> {
    storage
        .get_edges_for_node_ids(&[node_id])
        .ok()
        .and_then(|edges_by_node| edges_by_node.get(&node_id).cloned())
        .map(|edges| edge_digest_for_edges(&edges, limit))
        .unwrap_or_default()
}

pub(super) fn compact_doc_lines(lines: impl Iterator<Item = String>, limit: usize) -> Vec<String> {
    lines
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .take(limit)
        .collect()
}

pub(super) fn semantic_doc_budget_cost(token: &str) -> usize {
    token.chars().count().div_ceil(3).max(1)
}

#[cfg(test)]
pub(super) fn semantic_doc_text_budget_cost(doc_text: &str) -> usize {
    doc_text
        .split_whitespace()
        .map(semantic_doc_budget_cost)
        .sum()
}

pub(super) fn truncate_semantic_doc_text_to_token_budget(
    doc_text: &str,
    max_tokens: usize,
) -> String {
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

pub(super) fn comment_block_before(lines: &[&str], start_idx: usize, limit: usize) -> Vec<String> {
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

pub(super) fn symbol_excerpt(
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
pub(super) fn build_llm_symbol_doc_text(
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

pub(super) fn build_llm_symbol_doc_text_with_policy(
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

#[derive(Debug, Clone)]
pub(super) struct PendingLlmSymbolDoc {
    pub(super) node_id: codestory_contracts::graph::NodeId,
    pub(super) file_node_id: Option<codestory_contracts::graph::NodeId>,
    pub(super) kind: codestory_contracts::graph::NodeKind,
    pub(super) display_name: String,
    pub(super) qualified_name: Option<String>,
    pub(super) file_path: Option<String>,
    pub(super) start_line: Option<u32>,
    pub(super) end_line: Option<u32>,
    pub(super) doc_text: String,
    pub(super) doc_hash: String,
    pub(super) dense_reason: DenseAnchorReason,
}

#[derive(Debug)]
pub(super) struct BuiltLlmSymbolDoc {
    pub(super) symbol_doc: SymbolSearchDoc,
    pub(super) pending: Option<PendingLlmSymbolDoc>,
    pub(super) reusable: bool,
}

#[cfg(test)]
pub(super) fn llm_symbol_doc_hash(doc_text: &str) -> String {
    llm_symbol_doc_hash_with_alias(doc_text, semantic_doc_alias_mode_from_env())
}

pub(super) fn llm_symbol_doc_hash_with_alias(
    doc_text: &str,
    alias_mode: SemanticDocAliasMode,
) -> String {
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

pub(super) fn observe_dense_anchor_reason(
    stats: &mut SemanticProjectionStats,
    reason: DenseAnchorReason,
) {
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

pub(super) fn dense_anchor_score(
    graph_context: &SemanticDocGraphContext,
    node_id: GraphNodeId,
) -> usize {
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

pub(super) fn dense_anchor_is_central(
    graph_context: &SemanticDocGraphContext,
    node_id: GraphNodeId,
) -> bool {
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

pub(super) fn semantic_component_key_for_path(path: Option<&str>) -> Option<String> {
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

pub(super) fn virtual_component_report_node_id(component_key: &str) -> GraphNodeId {
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

pub(super) fn semantic_file_is_entrypoint(path: Option<&str>, display_name: &str) -> bool {
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

pub(super) fn semantic_path_is_entrypoint_file(path: Option<&str>) -> bool {
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

pub(super) fn semantic_file_is_public_surface(path: Option<&str>) -> bool {
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

pub(super) fn dense_anchor_public_kind(kind: codestory_contracts::graph::NodeKind) -> bool {
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

pub(super) fn dense_anchor_callable_kind(kind: codestory_contracts::graph::NodeKind) -> bool {
    matches!(
        kind,
        codestory_contracts::graph::NodeKind::FUNCTION
            | codestory_contracts::graph::NodeKind::METHOD
            | codestory_contracts::graph::NodeKind::MACRO
    )
}

pub(super) fn semantic_file_is_package_callable_surface(path: Option<&str>) -> bool {
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

pub(super) fn semantic_doc_is_documented_nontrivial(doc_text: &str) -> bool {
    if !doc_text.contains("comments:") {
        return false;
    }
    doc_text
        .lines()
        .find_map(|line| line.strip_prefix("body_summary:"))
        .is_some_and(|body| body.split_whitespace().count() >= 8)
}

pub(super) fn dense_anchor_reason_for_node(
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

pub(super) fn is_retrieval_artifact_node(node: &GraphNode) -> bool {
    node.serialized_name.starts_with("component_report:")
        || node
            .canonical_id
            .as_deref()
            .is_some_and(|canonical_id| canonical_id.starts_with("codestory:component_report:"))
}

#[cfg(test)]
pub(super) fn build_component_report_docs(
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
pub(super) struct ComponentReportNode {
    node: GraphNode,
    file_path: String,
    centrality: usize,
}

#[derive(Debug, Default)]
pub(super) struct ComponentReportSummary {
    symbol_count: usize,
    files: BTreeSet<String>,
    top_nodes: Vec<ComponentReportNode>,
}

#[derive(Debug, Default)]
pub(super) struct ComponentReportAccumulator {
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

pub(super) fn build_component_report_docs_with_policy(
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

pub(super) fn sort_pending_dense_anchor_inputs(docs: &mut [PendingLlmSymbolDoc]) {
    docs.sort_by_key(|doc| doc.node_id.0);
}

pub(super) fn flush_pending_dense_anchor_inputs(
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
pub(super) fn process_semantic_symbol_nodes(
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
pub(super) fn publish_component_report_docs(
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
pub(super) fn sync_llm_symbol_projection_for_runtime(
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
pub(super) enum SemanticProjectionDocumentSource {
    SourceFiles,
    StoredCore,
}

pub(super) fn sync_full_llm_symbol_projection_streaming_for_runtime(
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
pub(super) fn finalize_staged_semantic_docs(
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

pub(super) fn finalize_staged_semantic_docs_for_runtime(
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

pub(super) fn load_persisted_semantic_docs_for_runtime(
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
