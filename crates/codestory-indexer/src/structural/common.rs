use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{
    AccessKind, Edge, EdgeId, EdgeKind, Node, NodeId, NodeKind, Occurrence, OccurrenceKind,
    ResolutionCertainty, SourceLocation,
};
use codestory_contracts::language_support::is_github_actions_workflow_path;
use std::path::Path;

pub(crate) fn structural_language_name(path: &Path) -> &'static str {
    if is_github_actions_workflow_path(path.to_string_lossy().as_ref()) {
        return "github_actions_workflow";
    }
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "html",
        "css" => "css",
        "sql" => "sql",
        _ => "structural",
    }
}

pub(crate) fn push_member_edge(
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    parent_id: NodeId,
    child_id: NodeId,
    line: u32,
) {
    storage.edges.push(Edge {
        id: EdgeId(structural_edge_id(
            parent_id.0,
            child_id.0,
            EdgeKind::MEMBER,
        )),
        source: parent_id,
        target: child_id,
        kind: EdgeKind::MEMBER,
        file_node_id: Some(file_id),
        line: Some(line),
        certainty: Some(ResolutionCertainty::Certain),
        ..Default::default()
    });
}

pub(crate) fn push_usage_edge(
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    source_id: NodeId,
    target_id: NodeId,
    line: u32,
) {
    storage.edges.push(Edge {
        id: EdgeId(structural_edge_id(
            source_id.0,
            target_id.0,
            EdgeKind::USAGE,
        )),
        source: source_id,
        target: target_id,
        kind: EdgeKind::USAGE,
        file_node_id: Some(file_id),
        line: Some(line),
        certainty: Some(ResolutionCertainty::Certain),
        ..Default::default()
    });
}

pub(crate) fn push_import_edge(
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    source_id: NodeId,
    target_id: NodeId,
    line: u32,
) {
    storage.edges.push(Edge {
        id: EdgeId(structural_edge_id(
            source_id.0,
            target_id.0,
            EdgeKind::IMPORT,
        )),
        source: source_id,
        target: target_id,
        kind: EdgeKind::IMPORT,
        file_node_id: Some(file_id),
        line: Some(line),
        certainty: Some(ResolutionCertainty::Certain),
        ..Default::default()
    });
}

pub(crate) fn push_type_usage_edge(
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    source_id: NodeId,
    target_id: NodeId,
    line: u32,
) {
    storage.edges.push(Edge {
        id: EdgeId(structural_edge_id(
            source_id.0,
            target_id.0,
            EdgeKind::TYPE_USAGE,
        )),
        source: source_id,
        target: target_id,
        kind: EdgeKind::TYPE_USAGE,
        file_node_id: Some(file_id),
        line: Some(line),
        certainty: Some(ResolutionCertainty::Certain),
        ..Default::default()
    });
}

pub(crate) fn push_annotation_usage_edge(
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    source_id: NodeId,
    target_id: NodeId,
    line: u32,
) {
    storage.edges.push(Edge {
        id: EdgeId(structural_edge_id(
            source_id.0,
            target_id.0,
            EdgeKind::ANNOTATION_USAGE,
        )),
        source: source_id,
        target: target_id,
        kind: EdgeKind::ANNOTATION_USAGE,
        file_node_id: Some(file_id),
        line: Some(line),
        certainty: Some(ResolutionCertainty::Certain),
        ..Default::default()
    });
}

pub(crate) fn push_structural_node(
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    kind: NodeKind,
    name: &str,
    canonical_id: &str,
    line: u32,
    col: u32,
) -> NodeId {
    let node_id = NodeId(crate::generate_id(canonical_id));
    let label_len = name.len().max(1);
    let start_col = col.max(1);
    let end_col = start_col.saturating_add(label_len as u32).saturating_sub(1);
    storage.nodes.push(Node {
        id: node_id,
        kind,
        serialized_name: name.to_string(),
        qualified_name: Some(name.to_string()),
        canonical_id: Some(canonical_id.to_string()),
        file_node_id: Some(file_id),
        start_line: Some(line),
        start_col: Some(start_col),
        end_line: Some(line),
        end_col: Some(end_col),
    });
    storage.component_access.push((node_id, AccessKind::Public));
    storage.occurrences.push(Occurrence {
        element_id: node_id.0,
        kind: OccurrenceKind::DEFINITION,
        location: SourceLocation {
            file_node_id: file_id,
            start_line: line,
            start_col,
            end_line: line,
            end_col,
        },
    });
    node_id
}

fn structural_edge_id(source: i64, target: i64, kind: EdgeKind) -> i64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut update = |val: i64| {
        for b in val.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    update(source);
    update(target);
    update(kind as i64);
    h as i64
}
