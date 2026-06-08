use anyhow::{Context, Result};
use codestory_contracts::graph::{Edge, EdgeKind, Node, NodeId, NodeKind};
use codestory_store::Store;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_REPORT_LIMIT: usize = 10;

#[derive(Debug, Clone, Serialize)]
pub struct RepoReportExport {
    pub metadata: ReportGenerationMetadata,
    pub summary: RepoReportSummary,
    pub hotspots: Vec<ReportNodeSummary>,
    pub entry_points: Vec<ReportNodeSummary>,
    pub bridge_nodes: Vec<ReportNodeSummary>,
    pub follow_up_queries: Vec<ReportFollowUpQuery>,
    pub graph: GraphExport,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReportGenerationMetadata {
    pub format_version: u32,
    pub artifact_role: String,
    pub source: String,
    pub project_root: String,
    pub storage_path: String,
    pub generated_at_epoch_ms: u128,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoReportSummary {
    pub node_count: i64,
    pub edge_count: i64,
    pub file_count: i64,
    pub error_count: i64,
    pub exported_node_count: usize,
    pub exported_edge_count: usize,
    pub node_kinds: BTreeMap<String, usize>,
    pub edge_kinds: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReportNodeSummary {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub incoming_edges: usize,
    pub outgoing_edges: usize,
    pub total_edges: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_location: Option<SourceLocation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReportFollowUpQuery {
    pub query: String,
    pub reason: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphExport {
    pub nodes: Vec<GraphExportNode>,
    pub edges: Vec<GraphExportEdge>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphExportNode {
    pub id: i64,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_node_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_location: Option<SourceLocation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphExportEdge {
    pub id: i64,
    pub source: i64,
    pub target: i64,
    pub effective_source: i64,
    pub effective_target: i64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certainty: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callsite_identity: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidate_targets: Vec<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_location: Option<SourceLocation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceLocation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_col: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_col: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default)]
struct NodeDegree {
    incoming: usize,
    outgoing: usize,
}

impl NodeDegree {
    fn total(self) -> usize {
        self.incoming + self.outgoing
    }

    fn bridge_score(self) -> usize {
        self.incoming.min(self.outgoing)
    }
}

pub fn build_report_export(
    project_root: impl AsRef<Path>,
    storage_path: impl AsRef<Path>,
    limit: usize,
) -> Result<RepoReportExport> {
    let project_root = project_root.as_ref();
    let storage_path = storage_path.as_ref();
    let storage = Store::open(storage_path).with_context(|| {
        format!(
            "Failed to open CodeStory store at {}",
            storage_path.display()
        )
    })?;
    let stats = storage
        .get_stats()
        .context("Failed to query CodeStory store stats")?;
    let nodes = storage
        .get_nodes()
        .context("Failed to query CodeStory graph nodes")?;
    let edges = storage
        .get_edges()
        .context("Failed to query CodeStory graph edges")?;

    let limit = limit.max(1);
    let nodes_by_id = nodes
        .iter()
        .map(|node| (node.id, node.clone()))
        .collect::<HashMap<_, _>>();
    let degrees = degree_map(&edges);
    let summary = RepoReportSummary {
        node_count: stats.node_count,
        edge_count: stats.edge_count,
        file_count: stats.file_count,
        error_count: stats.error_count,
        exported_node_count: nodes.len(),
        exported_edge_count: edges.len(),
        node_kinds: node_kind_counts(&nodes),
        edge_kinds: edge_kind_counts(&edges),
    };

    let hotspots = top_hotspots(&nodes, &nodes_by_id, &degrees, limit);
    let entry_points = top_entry_points(&nodes, &nodes_by_id, &degrees, limit);
    let bridge_nodes = top_bridge_nodes(&nodes, &nodes_by_id, &degrees, limit);
    let follow_up_queries =
        follow_up_queries(project_root, &entry_points, &bridge_nodes, &hotspots, limit);
    let graph = GraphExport {
        nodes: nodes
            .iter()
            .map(|node| graph_export_node(node, &nodes_by_id))
            .collect(),
        edges: edges
            .iter()
            .map(|edge| graph_export_edge(edge, &nodes_by_id))
            .collect(),
    };

    Ok(RepoReportExport {
        metadata: ReportGenerationMetadata {
            format_version: 1,
            artifact_role: "derived_output".to_string(),
            source: "current_store".to_string(),
            project_root: project_root.to_string_lossy().to_string(),
            storage_path: storage_path.to_string_lossy().to_string(),
            generated_at_epoch_ms: generated_at_epoch_ms(),
            note: "Report/export artifacts are generated from the current SQLite store and are not source-of-truth state.".to_string(),
        },
        summary,
        hotspots,
        entry_points,
        bridge_nodes,
        follow_up_queries,
        graph,
    })
}

fn degree_map(edges: &[Edge]) -> HashMap<NodeId, NodeDegree> {
    let mut degrees = HashMap::<NodeId, NodeDegree>::new();
    for edge in edges {
        let (source, target) = edge.effective_endpoints();
        degrees.entry(source).or_default().outgoing += 1;
        degrees.entry(target).or_default().incoming += 1;
    }
    degrees
}

fn top_hotspots(
    nodes: &[Node],
    nodes_by_id: &HashMap<NodeId, Node>,
    degrees: &HashMap<NodeId, NodeDegree>,
    limit: usize,
) -> Vec<ReportNodeSummary> {
    let mut candidates = nodes
        .iter()
        .filter(|node| node.kind != NodeKind::FILE)
        .filter_map(|node| {
            let degree = degrees.get(&node.id).copied().unwrap_or_default();
            (degree.total() > 0).then(|| report_node_summary(node, nodes_by_id, degree))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .total_edges
            .cmp(&left.total_edges)
            .then_with(|| right.outgoing_edges.cmp(&left.outgoing_edges))
            .then_with(|| left.name.cmp(&right.name))
    });
    candidates.truncate(limit);
    candidates
}

fn top_entry_points(
    nodes: &[Node],
    nodes_by_id: &HashMap<NodeId, Node>,
    degrees: &HashMap<NodeId, NodeDegree>,
    limit: usize,
) -> Vec<ReportNodeSummary> {
    let mut candidates = nodes
        .iter()
        .filter(|node| node.kind != NodeKind::FILE)
        .filter_map(|node| {
            let degree = degrees.get(&node.id).copied().unwrap_or_default();
            (degree.outgoing > 0 && (degree.incoming == 0 || looks_like_entry_point(node)))
                .then(|| report_node_summary(node, nodes_by_id, degree))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .outgoing_edges
            .cmp(&left.outgoing_edges)
            .then_with(|| left.incoming_edges.cmp(&right.incoming_edges))
            .then_with(|| left.name.cmp(&right.name))
    });
    candidates.truncate(limit);
    if candidates.is_empty() {
        candidates = top_hotspots(nodes, nodes_by_id, degrees, limit)
            .into_iter()
            .filter(|node| matches!(node.kind.as_str(), "function" | "method" | "module"))
            .collect();
    }
    candidates
}

fn top_bridge_nodes(
    nodes: &[Node],
    nodes_by_id: &HashMap<NodeId, Node>,
    degrees: &HashMap<NodeId, NodeDegree>,
    limit: usize,
) -> Vec<ReportNodeSummary> {
    let mut candidates = nodes
        .iter()
        .filter(|node| node.kind != NodeKind::FILE)
        .filter_map(|node| {
            let degree = degrees.get(&node.id).copied().unwrap_or_default();
            (degree.incoming > 0 && degree.outgoing > 0)
                .then(|| (degree, report_node_summary(node, nodes_by_id, degree)))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(left_degree, left), (right_degree, right)| {
        right_degree
            .bridge_score()
            .cmp(&left_degree.bridge_score())
            .then_with(|| right.total_edges.cmp(&left.total_edges))
            .then_with(|| left.name.cmp(&right.name))
    });
    candidates
        .into_iter()
        .map(|(_, summary)| summary)
        .take(limit)
        .collect()
}

fn follow_up_queries(
    project_root: &Path,
    entry_points: &[ReportNodeSummary],
    bridge_nodes: &[ReportNodeSummary],
    hotspots: &[ReportNodeSummary],
    limit: usize,
) -> Vec<ReportFollowUpQuery> {
    let mut seen = HashSet::new();
    let mut queries = Vec::new();
    push_follow_ups(
        project_root,
        "entry point candidate",
        entry_points,
        &mut seen,
        &mut queries,
        limit,
    );
    push_follow_ups(
        project_root,
        "bridge or high-connectivity node",
        bridge_nodes,
        &mut seen,
        &mut queries,
        limit,
    );
    push_follow_ups(
        project_root,
        "hotspot with many indexed relationships",
        hotspots,
        &mut seen,
        &mut queries,
        limit,
    );
    queries.truncate(limit.min(DEFAULT_REPORT_LIMIT));
    queries
}

fn push_follow_ups(
    project_root: &Path,
    reason: &str,
    nodes: &[ReportNodeSummary],
    seen: &mut HashSet<i64>,
    queries: &mut Vec<ReportFollowUpQuery>,
    limit: usize,
) {
    for node in nodes {
        if queries.len() >= limit || !seen.insert(node.id) {
            continue;
        }
        queries.push(ReportFollowUpQuery {
            query: node.name.clone(),
            reason: reason.to_string(),
            command: format!(
                "codestory-cli trail --project {} --id {} --story --hide-speculative",
                shell_quote_path(project_root),
                node.id
            ),
        });
    }
}

fn report_node_summary(
    node: &Node,
    nodes_by_id: &HashMap<NodeId, Node>,
    degree: NodeDegree,
) -> ReportNodeSummary {
    ReportNodeSummary {
        id: node.id.0,
        name: node_display_name(node),
        kind: node_kind_label(node.kind),
        incoming_edges: degree.incoming,
        outgoing_edges: degree.outgoing,
        total_edges: degree.total(),
        source_location: source_location_for_node(node, nodes_by_id),
    }
}

fn graph_export_node(node: &Node, nodes_by_id: &HashMap<NodeId, Node>) -> GraphExportNode {
    GraphExportNode {
        id: node.id.0,
        name: node_display_name(node),
        kind: node_kind_label(node.kind),
        qualified_name: node.qualified_name.clone(),
        canonical_id: node.canonical_id.clone(),
        file_node_id: node.file_node_id.map(|id| id.0),
        source_location: source_location_for_node(node, nodes_by_id),
    }
}

fn graph_export_edge(edge: &Edge, nodes_by_id: &HashMap<NodeId, Node>) -> GraphExportEdge {
    GraphExportEdge {
        id: edge.id.0,
        source: edge.source.0,
        target: edge.target.0,
        effective_source: edge.effective_source().0,
        effective_target: edge.effective_target().0,
        kind: edge_kind_label(edge.kind),
        confidence: edge.confidence,
        certainty: edge
            .certainty
            .map(|certainty| certainty.as_str().to_string()),
        callsite_identity: edge.callsite_identity.clone(),
        candidate_targets: edge
            .candidate_targets
            .iter()
            .map(|candidate| candidate.0)
            .collect(),
        source_location: source_location_for_edge(edge, nodes_by_id),
    }
}

fn source_location_for_node(
    node: &Node,
    nodes_by_id: &HashMap<NodeId, Node>,
) -> Option<SourceLocation> {
    let file = if node.kind == NodeKind::FILE {
        Some(node_display_name(node))
    } else {
        node.file_node_id
            .and_then(|id| nodes_by_id.get(&id))
            .map(node_display_name)
    };
    source_location(
        file,
        node.start_line,
        node.start_col,
        node.end_line,
        node.end_col,
    )
}

fn source_location_for_edge(
    edge: &Edge,
    nodes_by_id: &HashMap<NodeId, Node>,
) -> Option<SourceLocation> {
    let file = edge
        .file_node_id
        .and_then(|id| nodes_by_id.get(&id))
        .map(node_display_name);
    source_location(file, edge.line, None, None, None)
}

fn source_location(
    file: Option<String>,
    start_line: Option<u32>,
    start_col: Option<u32>,
    end_line: Option<u32>,
    end_col: Option<u32>,
) -> Option<SourceLocation> {
    (file.is_some()
        || start_line.is_some()
        || start_col.is_some()
        || end_line.is_some()
        || end_col.is_some())
    .then_some(SourceLocation {
        file,
        start_line,
        start_col,
        end_line,
        end_col,
    })
}

fn node_display_name(node: &Node) -> String {
    node.qualified_name
        .clone()
        .unwrap_or_else(|| node.serialized_name.clone())
}

fn looks_like_entry_point(node: &Node) -> bool {
    let name = node_display_name(node).to_ascii_lowercase();
    matches!(
        node.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MODULE
    ) && (name == "main"
        || name.ends_with("::main")
        || name.ends_with(".main")
        || name.contains("run")
        || name.contains("start")
        || name.contains("serve")
        || name.contains("index"))
}

fn node_kind_counts(nodes: &[Node]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for node in nodes {
        *counts.entry(node_kind_label(node.kind)).or_default() += 1;
    }
    counts
}

fn edge_kind_counts(edges: &[Edge]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for edge in edges {
        *counts.entry(edge_kind_label(edge.kind)).or_default() += 1;
    }
    counts
}

fn node_kind_label(kind: NodeKind) -> String {
    format!("{kind:?}").to_ascii_lowercase()
}

fn edge_kind_label(kind: EdgeKind) -> String {
    format!("{kind:?}").to_ascii_lowercase()
}

fn generated_at_epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn shell_quote_path(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    if raw.contains(' ') {
        format!("\"{}\"", raw.replace('"', "\\\""))
    } else {
        raw
    }
}
