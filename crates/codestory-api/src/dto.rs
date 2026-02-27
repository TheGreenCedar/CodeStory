use crate::ids::{EdgeId, NodeId};
use crate::types::{
    EdgeKind, IndexMode, LayoutDirection, MemberAccess, NodeKind, TrailCallerScope, TrailDirection,
    TrailMode,
};
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OpenProjectRequest {
    pub path: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct StartIndexingRequest {
    pub mode: IndexMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchRequest {
    pub query: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchHit {
    pub node_id: NodeId,
    pub display_name: String,
    pub kind: NodeKind,
    pub file_path: Option<String>,
    pub line: Option<u32>,
    pub score: f32,
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
pub struct ReadFileTextRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ReadFileTextResponse {
    pub path: String,
    pub text: String,
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
    pub connection: AgentConnectionSettingsDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentCitationDto {
    pub node_id: NodeId,
    pub display_name: String,
    pub kind: NodeKind,
    pub file_path: Option<String>,
    pub line: Option<u32>,
    pub score: f32,
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
