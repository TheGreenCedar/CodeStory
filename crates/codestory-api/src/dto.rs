use crate::ids::{EdgeId, NodeId};
use crate::types::{EdgeKind, IndexMode, NodeKind, TrailDirection, TrailMode};
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
    pub edge_filter: Vec<EdgeKind>,
    #[serde(default)]
    pub node_filter: Vec<NodeKind>,
    pub max_nodes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NodeDetailsRequest {
    pub id: NodeId,
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

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentAskRequest {
    pub prompt: String,
    #[serde(default)]
    pub include_mermaid: bool,
    #[serde(default)]
    pub focus_node_id: Option<NodeId>,
    #[serde(default)]
    pub max_results: Option<u32>,
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
pub struct AgentResponseSectionDto {
    pub id: String,
    pub title: String,
    pub markdown: String,
    pub graph_ids: Vec<String>,
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
pub struct AgentAnswerDto {
    pub prompt: String,
    pub summary: String,
    pub sections: Vec<AgentResponseSectionDto>,
    pub citations: Vec<AgentCitationDto>,
    pub graphs: Vec<GraphArtifactDto>,
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
