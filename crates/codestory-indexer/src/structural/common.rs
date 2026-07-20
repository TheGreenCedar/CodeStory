use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{
    AccessKind, Edge, EdgeId, EdgeKind, Node, NodeId, NodeKind, Occurrence, OccurrenceKind,
    ResolutionCertainty, SourceLocation,
};
use codestory_contracts::language_support::{
    is_cargo_manifest_file_path, is_docker_compose_file_path, is_github_actions_workflow_path,
    structural_language_name_for_path,
};
use std::path::Path;

pub(crate) fn structural_language_name(path: &Path) -> &'static str {
    let path = path.to_string_lossy();
    if is_github_actions_workflow_path(path.as_ref()) {
        return "github_actions_workflow";
    }
    if is_docker_compose_file_path(path.as_ref()) {
        return "docker_compose";
    }
    if is_cargo_manifest_file_path(path.as_ref()) {
        return "cargo_manifest";
    }
    structural_language_name_for_path(Some(path.as_ref())).unwrap_or("structural")
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StructuralSourceSpan {
    pub(crate) start_line: u32,
    pub(crate) start_col: u32,
    pub(crate) end_line: u32,
    pub(crate) end_col: u32,
}

impl StructuralSourceSpan {
    pub(crate) fn token(line: u32, zero_based_start: usize, byte_len: usize) -> Self {
        debug_assert!(line > 0);
        debug_assert!(byte_len > 0);
        Self {
            start_line: line,
            start_col: zero_based_start
                .saturating_add(1)
                .try_into()
                .unwrap_or(u32::MAX),
            end_line: line,
            end_col: zero_based_start
                .saturating_add(byte_len)
                .try_into()
                .unwrap_or(u32::MAX),
        }
    }
}

pub(crate) fn push_structural_node(
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    kind: NodeKind,
    name: &str,
    canonical_id: &str,
    span: StructuralSourceSpan,
) -> NodeId {
    let node_id = structural_node_id(file_id, canonical_id, span.start_line, span.start_col);
    storage.nodes.push(Node {
        id: node_id,
        kind,
        serialized_name: name.to_string(),
        qualified_name: Some(name.to_string()),
        canonical_id: Some(canonical_id.to_string()),
        file_node_id: Some(file_id),
        start_line: Some(span.start_line),
        start_col: Some(span.start_col),
        end_line: Some(span.end_line),
        end_col: Some(span.end_col),
    });
    storage.structural_unit_node_ids.push(node_id);
    storage.component_access.push((node_id, AccessKind::Public));
    storage.occurrences.push(Occurrence {
        element_id: node_id.0,
        kind: OccurrenceKind::DEFINITION,
        location: SourceLocation {
            file_node_id: file_id,
            start_line: span.start_line,
            start_col: span.start_col,
            end_line: span.end_line,
            end_col: span.end_col,
        },
    });
    node_id
}

pub(crate) fn push_synthetic_structural_node(
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    kind: NodeKind,
    name: &str,
    canonical_id: &str,
) -> NodeId {
    let node_id = structural_node_id(file_id, canonical_id, 0, 0);
    storage.nodes.push(Node {
        id: node_id,
        kind,
        serialized_name: name.to_string(),
        qualified_name: Some(name.to_string()),
        canonical_id: Some(canonical_id.to_string()),
        file_node_id: Some(file_id),
        start_line: None,
        start_col: None,
        end_line: None,
        end_col: None,
    });
    storage.component_access.push((node_id, AccessKind::Public));
    node_id
}

fn structural_node_id(file_id: NodeId, canonical_id: &str, line: u32, col: u32) -> NodeId {
    NodeId(crate::generate_id(&format!(
        "structural:{}:{line}:{col}:{canonical_id}",
        file_id.0,
    )))
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
