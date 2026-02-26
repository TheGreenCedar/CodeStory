use codestory_core::{AccessKind, Edge, ErrorInfo, Node, NodeId, Occurrence};

#[derive(Default)]
pub struct IntermediateStorage {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub occurrences: Vec<Occurrence>,
    pub component_access: Vec<(NodeId, AccessKind)>,
    pub errors: Vec<ErrorInfo>,
}

impl IntermediateStorage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, node: Node) {
        self.nodes.push(node);
    }

    pub fn add_edge(&mut self, edge: Edge) {
        self.edges.push(edge);
    }

    pub fn add_occurrence(&mut self, occurrence: Occurrence) {
        self.occurrences.push(occurrence);
    }

    pub fn add_error(&mut self, error: ErrorInfo) {
        self.errors.push(error);
    }

    pub fn add_component_access(&mut self, node_id: NodeId, access: AccessKind) {
        self.component_access.push((node_id, access));
    }

    pub fn merge(&mut self, other: IntermediateStorage) {
        self.nodes.extend(other.nodes);
        self.edges.extend(other.edges);
        self.occurrences.extend(other.occurrences);
        self.component_access.extend(other.component_access);
        self.errors.extend(other.errors);
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.edges.clear();
        self.occurrences.clear();
        self.component_access.clear();
        self.errors.clear();
    }
}
