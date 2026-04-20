use super::ids::{EdgeId, NodeId};
use super::types::{
    EdgeKind, IndexMode, LayoutDirection, MemberAccess, NodeKind, TrailCallerScope, TrailDirection,
    TrailMode,
};
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OpenProjectRequest {
    pub path: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum GroundingBudgetDto {
    Strict,
    #[default]
    Balanced,
    Max,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct StorageStatsDto {
    // Use u32 so TS can safely represent these as `number` without BigInt.
    pub node_count: u32,
    pub edge_count: u32,
    pub file_count: u32,
    pub error_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ProjectSummary {
    pub root: String,
    pub stats: StorageStatsDto,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<WorkspaceMemberIndexDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval: Option<RetrievalStateDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct StartIndexingRequest {
    pub mode: IndexMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchRepoTextMode {
    #[default]
    Auto,
    On,
    Off,
}

fn default_search_limit_per_source() -> u32 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub repo_text: SearchRepoTextMode,
    #[serde(default = "default_search_limit_per_source")]
    pub limit_per_source: u32,
    #[serde(default)]
    pub hybrid_weights: Option<AgentHybridWeightsDto>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchHitOrigin {
    IndexedSymbol,
    TextMatch,
}

impl SearchHitOrigin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IndexedSymbol => "indexed_symbol",
            Self::TextMatch => "text_match",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalModeDto {
    Hybrid,
    Symbolic,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalFallbackReasonDto {
    DisabledByConfig,
    MissingEmbeddingRuntime,
    MissingSemanticDocs,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RetrievalStateDto {
    pub mode: RetrievalModeDto,
    pub hybrid_configured: bool,
    pub semantic_ready: bool,
    pub semantic_doc_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<RetrievalFallbackReasonDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchHit {
    pub node_id: NodeId,
    pub display_name: String,
    pub kind: NodeKind,
    pub file_path: Option<String>,
    pub line: Option<u32>,
    pub score: f32,
    pub origin: SearchHitOrigin,
    pub resolvable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_breakdown: Option<RetrievalScoreBreakdownDto>,
}

impl SearchHit {
    pub const fn is_text_match(&self) -> bool {
        matches!(self.origin, SearchHitOrigin::TextMatch)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchResultsDto {
    pub query: String,
    pub retrieval: RetrievalStateDto,
    pub limit_per_source: u32,
    pub repo_text_mode: SearchRepoTextMode,
    pub repo_text_enabled: bool,
    #[serde(default)]
    pub suggestions: Vec<SearchHit>,
    #[serde(default)]
    pub indexed_symbol_hits: Vec<SearchHit>,
    #[serde(default)]
    pub repo_text_hits: Vec<SearchHit>,
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ListRootSymbolsRequest {
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ListChildrenSymbolsRequest {
    pub parent_id: NodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SymbolSummaryDto {
    pub id: NodeId,
    pub label: String,
    pub kind: NodeKind,
    pub file_path: Option<String>,
    pub has_children: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GroundingSymbolDigestDto {
    pub id: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_ref: Option<String>,
    pub label: String,
    pub kind: NodeKind,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub member_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub edge_digest: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GroundingFileDigestDto {
    pub file_path: String,
    #[serde(default)]
    pub language: Option<String>,
    pub symbol_count: u32,
    pub represented_symbol_count: u32,
    pub compressed: bool,
    pub symbols: Vec<GroundingSymbolDigestDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GroundingCoverageBucketDto {
    pub label: String,
    pub file_count: u32,
    pub symbol_count: u32,
    #[serde(default)]
    pub sample_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GroundingCoverageDto {
    pub total_files: u32,
    pub represented_files: u32,
    pub total_symbols: u32,
    pub represented_symbols: u32,
    pub compressed_files: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GroundingSnapshotDto {
    pub root: String,
    pub budget: GroundingBudgetDto,
    pub generated_at_epoch_ms: i64,
    pub stats: StorageStatsDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval: Option<RetrievalStateDto>,
    pub coverage: GroundingCoverageDto,
    pub root_symbols: Vec<GroundingSymbolDigestDto>,
    pub files: Vec<GroundingFileDigestDto>,
    #[serde(default)]
    pub coverage_buckets: Vec<GroundingCoverageBucketDto>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub recommended_queries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GraphRequest {
    pub center_id: NodeId,
    /// Optional cap to avoid pulling extremely dense neighborhoods into the UI.
    pub max_edges: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GraphNodeDto {
    pub id: NodeId,
    pub label: String,
    pub kind: NodeKind,
    pub depth: u32,
    #[serde(default)]
    pub label_policy: Option<String>,
    #[serde(default)]
    pub badge_visible_members: Option<u32>,
    #[serde(default)]
    pub badge_total_members: Option<u32>,
    #[serde(default)]
    pub merged_symbol_examples: Vec<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub qualified_name: Option<String>,
    #[serde(default)]
    pub member_access: Option<MemberAccess>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GraphEdgeDto {
    pub id: EdgeId,
    pub source: NodeId,
    pub target: NodeId,
    pub kind: EdgeKind,
    #[serde(default)]
    pub confidence: Option<f32>,
    /// `certain`, `probable`, or `uncertain`.
    #[serde(default)]
    pub certainty: Option<String>,
    #[serde(default)]
    pub callsite_identity: Option<String>,
    #[serde(default)]
    pub candidate_targets: Vec<NodeId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GraphResponse {
    pub center_id: NodeId,
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
    pub truncated: bool,
    #[serde(default)]
    pub omitted_edge_count: u32,
    #[serde(default)]
    pub canonical_layout: Option<CanonicalLayoutDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CanonicalLayoutDto {
    pub schema_version: u32,
    pub center_node_id: NodeId,
    pub nodes: Vec<CanonicalNodeDto>,
    pub edges: Vec<CanonicalEdgeDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CanonicalNodeDto {
    pub id: NodeId,
    pub kind: NodeKind,
    pub label: String,
    pub center: bool,
    pub node_style: CanonicalNodeStyle,
    pub is_non_indexed: bool,
    pub duplicate_count: u32,
    #[serde(default)]
    pub merged_symbol_ids: Vec<NodeId>,
    pub member_count: u32,
    #[serde(default)]
    pub badge_visible_members: Option<u32>,
    #[serde(default)]
    pub badge_total_members: Option<u32>,
    #[serde(default)]
    pub members: Vec<CanonicalMemberDto>,
    pub x_rank: i32,
    pub y_rank: u32,
    pub width: f32,
    pub height: f32,
    pub is_virtual_bundle: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CanonicalEdgeDto {
    pub id: String,
    #[serde(default)]
    pub source_edge_ids: Vec<EdgeId>,
    pub source: NodeId,
    pub target: NodeId,
    pub source_handle: String,
    pub target_handle: String,
    pub kind: EdgeKind,
    #[serde(default)]
    pub certainty: Option<String>,
    pub multiplicity: u32,
    pub family: CanonicalEdgeFamily,
    pub route_kind: CanonicalRouteKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CanonicalMemberDto {
    pub id: NodeId,
    pub label: String,
    pub kind: NodeKind,
    pub visibility: CanonicalMemberVisibility,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalNodeStyle {
    Card,
    Pill,
    Bundle,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalEdgeFamily {
    Flow,
    Hierarchy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalRouteKind {
    Direct,
    Hierarchy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalMemberVisibility {
    Public,
    Protected,
    Private,
    Default,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct TrailConfigDto {
    pub root_id: NodeId,
    #[serde(default)]
    pub mode: TrailMode,
    #[serde(default)]
    pub target_id: Option<NodeId>,
    /// Use `0` to mean "infinite" (bounded by `max_nodes`).
    pub depth: u32,
    pub direction: TrailDirection,
    #[serde(default)]
    pub caller_scope: TrailCallerScope,
    pub edge_filter: Vec<EdgeKind>,
    #[serde(default = "default_show_utility_calls")]
    pub show_utility_calls: bool,
    #[serde(default)]
    pub node_filter: Vec<NodeKind>,
    pub max_nodes: u32,
    #[serde(default = "default_layout_direction")]
    pub layout_direction: LayoutDirection,
}

const fn default_show_utility_calls() -> bool {
    false
}

const fn default_layout_direction() -> LayoutDirection {
    LayoutDirection::Horizontal
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct TrailFilterOptionsDto {
    pub node_kinds: Vec<NodeKind>,
    pub edge_kinds: Vec<EdgeKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct TrailContextDto {
    pub focus: NodeDetailsDto,
    pub trail: GraphResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NodeDetailsRequest {
    pub id: NodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NodeOccurrencesRequest {
    pub id: NodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct EdgeOccurrencesRequest {
    pub id: EdgeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SourceOccurrenceDto {
    pub element_id: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NodeDetailsDto {
    pub id: NodeId,
    pub kind: NodeKind,
    pub display_name: String,
    pub serialized_name: String,
    pub qualified_name: Option<String>,
    pub canonical_id: Option<String>,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub start_col: Option<u32>,
    pub end_line: Option<u32>,
    pub end_col: Option<u32>,
    #[serde(default)]
    pub member_access: Option<MemberAccess>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SymbolContextDto {
    pub node: NodeDetailsDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub children: Vec<SymbolSummaryDto>,
    #[serde(default)]
    pub related_hits: Vec<SearchHit>,
    #[serde(default)]
    pub edge_digest: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ReadFileTextRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ReadFileTextResponse {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SnippetContextDto {
    pub node: NodeDetailsDto,
    pub path: String,
    pub line: u32,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IndexDryRunDto {
    pub root: String,
    pub storage_path: String,
    pub refresh: IndexMode,
    pub files_to_index: u32,
    pub files_to_remove: u32,
    #[serde(default)]
    pub sample_files_to_index: Vec<String>,
    #[serde(default)]
    pub sample_file_ids_to_remove: Vec<i64>,
    #[serde(default)]
    pub members: Vec<WorkspaceMemberIndexDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WorkspaceMemberIndexDto {
    pub path: String,
    pub files_to_index: u32,
    pub indexed_files: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SummaryGenerationDto {
    pub generated: u32,
    pub reused: u32,
    pub skipped: u32,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WriteFileTextRequest {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SetUiLayoutRequest {
    pub json: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentBackend {
    #[default]
    Codex,
    ClaudeCode,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Default)]
pub struct AgentConnectionSettingsDto {
    #[serde(default)]
    pub backend: AgentBackend,
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRetrievalPresetDto {
    #[default]
    Architecture,
    Callflow,
    Inheritance,
    Impact,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRetrievalPolicyModeDto {
    LatencyFirst,
    CompletenessFirst,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentCustomRetrievalConfigDto {
    /// Use `0` to mean "infinite" (bounded by `max_nodes`).
    #[serde(default = "default_custom_depth")]
    pub depth: u32,
    #[serde(default = "default_custom_direction")]
    pub direction: TrailDirection,
    #[serde(default)]
    pub edge_filter: Vec<EdgeKind>,
    #[serde(default)]
    pub node_filter: Vec<NodeKind>,
    #[serde(default = "default_custom_max_nodes")]
    pub max_nodes: u32,
    #[serde(default)]
    pub include_edge_occurrences: bool,
    #[serde(default = "default_custom_enable_source_reads")]
    pub enable_source_reads: bool,
}

const fn default_custom_depth() -> u32 {
    3
}

const fn default_custom_direction() -> TrailDirection {
    TrailDirection::Both
}

const fn default_custom_max_nodes() -> u32 {
    800
}

const fn default_custom_enable_source_reads() -> bool {
    true
}

impl Default for AgentCustomRetrievalConfigDto {
    fn default() -> Self {
        Self {
            depth: default_custom_depth(),
            direction: default_custom_direction(),
            edge_filter: Vec::new(),
            node_filter: Vec::new(),
            max_nodes: default_custom_max_nodes(),
            include_edge_occurrences: false,
            enable_source_reads: default_custom_enable_source_reads(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentRetrievalProfileSelectionDto {
    #[default]
    Auto,
    Preset {
        preset: AgentRetrievalPresetDto,
    },
    Custom {
        config: AgentCustomRetrievalConfigDto,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentResponseModeDto {
    #[default]
    Markdown,
    Structured,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Default)]
pub struct AgentHybridWeightsDto {
    #[serde(default)]
    pub lexical: Option<f32>,
    #[serde(default)]
    pub semantic: Option<f32>,
    #[serde(default)]
    pub graph: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentAskRequest {
    pub prompt: String,
    #[serde(default)]
    pub retrieval_profile: AgentRetrievalProfileSelectionDto,
    #[serde(default)]
    pub focus_node_id: Option<NodeId>,
    #[serde(default)]
    pub max_results: Option<u32>,
    #[serde(default)]
    pub response_mode: AgentResponseModeDto,
    #[serde(default)]
    pub latency_budget_ms: Option<u32>,
    #[serde(default = "default_include_evidence")]
    pub include_evidence: bool,
    #[serde(default)]
    pub hybrid_weights: Option<AgentHybridWeightsDto>,
    #[serde(default)]
    pub connection: AgentConnectionSettingsDto,
    #[serde(default = "default_run_local_agent")]
    pub run_local_agent: bool,
}

const fn default_include_evidence() -> bool {
    true
}

const fn default_run_local_agent() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RetrievalScoreBreakdownDto {
    pub lexical: f32,
    pub semantic: f32,
    pub graph: f32,
    pub total: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentCitationDto {
    pub node_id: NodeId,
    pub display_name: String,
    pub kind: NodeKind,
    pub file_path: Option<String>,
    pub line: Option<u32>,
    pub score: f32,
    #[serde(default)]
    pub subgraph_id: Option<String>,
    #[serde(default)]
    pub evidence_edge_ids: Vec<EdgeId>,
    #[serde(default)]
    pub retrieval_score_breakdown: Option<RetrievalScoreBreakdownDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentResponseBlockDto {
    Markdown { markdown: String },
    Mermaid { graph_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentResponseSectionDto {
    pub id: String,
    pub title: String,
    pub blocks: Vec<AgentResponseBlockDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GraphArtifactDto {
    Uml {
        id: String,
        title: String,
        graph: GraphResponse,
    },
    Mermaid {
        id: String,
        title: String,
        diagram: String,
        mermaid_syntax: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentRetrievalSummaryFieldDto {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRetrievalStepKindDto {
    Search,
    SemanticQueryEmbedding,
    SemanticCandidateRetrieval,
    HybridRerank,
    TrailFilterOptions,
    Neighborhood,
    Trail,
    NodeDetails,
    NodeOccurrences,
    EdgeOccurrences,
    SourceRead,
    MermaidSynthesis,
    AnswerSynthesis,
    LocalAgent,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRetrievalStepStatusDto {
    Ok,
    Error,
    Skipped,
    Truncated,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentRetrievalStepDto {
    pub kind: AgentRetrievalStepKindDto,
    pub status: AgentRetrievalStepStatusDto,
    pub duration_ms: u32,
    #[serde(default)]
    pub input: Vec<AgentRetrievalSummaryFieldDto>,
    #[serde(default)]
    pub output: Vec<AgentRetrievalSummaryFieldDto>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentRetrievalTraceDto {
    pub request_id: String,
    pub resolved_profile: AgentRetrievalPresetDto,
    pub policy_mode: AgentRetrievalPolicyModeDto,
    pub total_latency_ms: u32,
    #[serde(default)]
    pub sla_target_ms: Option<u32>,
    #[serde(default)]
    pub sla_missed: bool,
    #[serde(default)]
    pub annotations: Vec<String>,
    pub steps: Vec<AgentRetrievalStepDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentAnswerDto {
    pub answer_id: String,
    pub prompt: String,
    pub summary: String,
    pub sections: Vec<AgentResponseSectionDto>,
    pub citations: Vec<AgentCitationDto>,
    pub subgraph_ids: Vec<String>,
    pub retrieval_version: String,
    pub graphs: Vec<GraphArtifactDto>,
    pub retrieval_trace: AgentRetrievalTraceDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct BookmarkCategoryDto {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CreateBookmarkCategoryRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct UpdateBookmarkCategoryRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct BookmarkDto {
    pub id: String,
    pub category_id: String,
    pub node_id: NodeId,
    pub comment: Option<String>,
    pub node_label: String,
    pub node_kind: NodeKind,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CreateBookmarkRequest {
    pub category_id: String,
    pub node_id: NodeId,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct UpdateBookmarkRequest {
    #[serde(default)]
    pub category_id: Option<String>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub comment: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OpenDefinitionRequest {
    pub node_id: NodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OpenContainingFolderRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SystemActionResponse {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WriteFileDataUrlRequest {
    pub path: String,
    /// A `data:*;base64,...` URL (for example from `graph.toDataURL()`).
    pub data_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WriteFileResponse {
    // Use u32 so TS can safely represent this as `number` without BigInt.
    pub bytes_written: u32,
}
