use codestory_contracts::graph::{
    AccessKind, CallableProjectionState, Edge, ErrorInfo, Node, NodeId, Occurrence,
};

/// Mutable projection accumulator used before flushing to storage.
///
/// Parser-backed indexers and structural collectors both fill this shape. A
/// caller should merge or flush it as one coherent projection so file rows,
/// nodes, edges, occurrences, and callable projection state stay aligned.
#[derive(Default)]
pub struct IntermediateStorage {
    pub files: Vec<codestory_store::FileInfo>,
    pub file_content_hashes: Vec<codestory_store::FileContentHash>,
    pub nodes: Vec<Node>,
    pub structural_unit_node_ids: Vec<NodeId>,
    pub structural_text_units: Vec<codestory_store::StructuralTextUnit>,
    pub structural_text_projections: Vec<codestory_store::StructuralTextProjection>,
    pub structural_text_cache_writes: Vec<StructuralTextArtifactCacheWrite>,
    pub edges: Vec<Edge>,
    pub occurrences: Vec<Occurrence>,
    pub component_access: Vec<(NodeId, AccessKind)>,
    pub callable_projection_states: Vec<CallableProjectionState>,
    pub impl_anchor_node_ids: Vec<NodeId>,
    pub errors: Vec<ErrorInfo>,
}

pub struct StructuralTextArtifactCacheWrite {
    pub path: std::path::PathBuf,
    pub cache_key: String,
    pub artifact_blob: Vec<u8>,
}

impl IntermediateStorage {
    /// Create an empty projection accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one graph node.
    pub fn add_node(&mut self, node: Node) {
        self.nodes.push(node);
    }

    /// Append one graph edge.
    pub fn add_edge(&mut self, edge: Edge) {
        self.edges.push(edge);
    }

    /// Append one source occurrence.
    pub fn add_occurrence(&mut self, occurrence: Occurrence) {
        self.occurrences.push(occurrence);
    }

    /// Append one indexing error.
    pub fn add_error(&mut self, error: ErrorInfo) {
        self.errors.push(error);
    }

    /// Append component-access metadata for a node.
    pub fn add_component_access(&mut self, node_id: NodeId, access: AccessKind) {
        self.component_access.push((node_id, access));
    }

    /// Merge another accumulator into this one, preserving insertion order.
    pub fn merge(&mut self, other: IntermediateStorage) {
        self.files.extend(other.files);
        self.file_content_hashes.extend(other.file_content_hashes);
        self.nodes.extend(other.nodes);
        self.structural_unit_node_ids
            .extend(other.structural_unit_node_ids);
        self.structural_text_units
            .extend(other.structural_text_units);
        self.structural_text_projections
            .extend(other.structural_text_projections);
        self.structural_text_cache_writes
            .extend(other.structural_text_cache_writes);
        self.edges.extend(other.edges);
        self.occurrences.extend(other.occurrences);
        self.component_access.extend(other.component_access);
        self.callable_projection_states
            .extend(other.callable_projection_states);
        self.impl_anchor_node_ids.extend(other.impl_anchor_node_ids);
        self.errors.extend(other.errors);
    }

    /// Remove all accumulated projection data and errors.
    pub fn clear(&mut self) {
        self.files.clear();
        self.file_content_hashes.clear();
        self.nodes.clear();
        self.structural_unit_node_ids.clear();
        self.structural_text_units.clear();
        self.structural_text_projections.clear();
        self.structural_text_cache_writes.clear();
        self.edges.clear();
        self.occurrences.clear();
        self.component_access.clear();
        self.callable_projection_states.clear();
        self.impl_anchor_node_ids.clear();
        self.errors.clear();
    }
}
