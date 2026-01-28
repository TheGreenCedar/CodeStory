use codestory_core::{Edge, ErrorInfo, Node, Occurrence};

#[derive(Default)]
pub struct IntermediateStorage {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub occurrences: Vec<Occurrence>,
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

    pub fn merge(&mut self, other: IntermediateStorage) {
        self.nodes.extend(other.nodes);
        self.edges.extend(other.edges);
        self.occurrences.extend(other.occurrences);
        self.errors.extend(other.errors);
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.edges.clear();
        self.occurrences.clear();
        self.errors.clear();
    }
}
