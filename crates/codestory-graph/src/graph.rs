use codestory_core::{BundleInfo, Edge, EdgeId, Node, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::ops::{Index, IndexMut};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeIndex(pub usize);

impl fmt::Display for NodeIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdgeIndex(pub usize);

impl fmt::Display for EdgeIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupType {
    FILE,
    NAMESPACE,
    INHERITANCE,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupLayout {
    GRID,
    LIST,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DummyNode {
    pub id: NodeId,
    pub node_kind: codestory_core::NodeKind,
    pub name: String,

    // Visual properties
    pub position: Vec2,
    pub size: Vec2,
    pub visible: bool,
    pub active: bool,
    pub focused: bool,
    pub expanded: bool,

    // Hierarchy
    pub children: Vec<NodeId>,
    pub parent: Option<NodeId>,

    // Bundling
    pub bundle_info: Option<BundleInfo>,
    pub bundled_nodes: Vec<NodeId>,

    // Grouping
    pub group_type: Option<GroupType>,
    pub group_layout: GroupLayout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DummyEdge {
    pub id: EdgeId,
    pub source: NodeId,
    pub target: NodeId,
    pub kind: codestory_core::EdgeKind,
    pub visible: bool,
    pub active: bool,
    pub source_idx: NodeIndex,
    pub target_idx: NodeIndex,
}

#[derive(Debug)]
pub struct Graph {
    nodes: Vec<DummyNode>,
    edges: Vec<DummyEdge>,
}

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}

impl Graph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn add_node(&mut self, node: DummyNode) -> NodeIndex {
        let idx = NodeIndex(self.nodes.len());
        self.nodes.push(node);
        idx
    }

    pub fn add_edge(
        &mut self,
        source_idx: NodeIndex,
        target_idx: NodeIndex,
        edge: DummyEdge,
    ) -> EdgeIndex {
        let idx = EdgeIndex(self.edges.len());
        // Ensure edge has correct indices
        let mut edge = edge;
        edge.source_idx = source_idx;
        edge.target_idx = target_idx;
        self.edges.push(edge);
        idx
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn node_indices(&self) -> impl Iterator<Item = NodeIndex> {
        (0..self.nodes.len()).map(NodeIndex)
    }

    pub fn edge_indices(&self) -> impl Iterator<Item = EdgeIndex> {
        (0..self.edges.len()).map(EdgeIndex)
    }

    pub fn edge_endpoints(&self, index: EdgeIndex) -> Option<(NodeIndex, NodeIndex)> {
        self.edges
            .get(index.0)
            .map(|e| (e.source_idx, e.target_idx))
    }

    pub fn node_weight(&self, index: NodeIndex) -> Option<&DummyNode> {
        self.nodes.get(index.0)
    }

    pub fn edge_weight(&self, index: EdgeIndex) -> Option<&DummyEdge> {
        self.edges.get(index.0)
    }
}

impl Index<NodeIndex> for Graph {
    type Output = DummyNode;
    fn index(&self, index: NodeIndex) -> &Self::Output {
        &self.nodes[index.0]
    }
}

impl IndexMut<NodeIndex> for Graph {
    fn index_mut(&mut self, index: NodeIndex) -> &mut Self::Output {
        &mut self.nodes[index.0]
    }
}

impl Index<EdgeIndex> for Graph {
    type Output = DummyEdge;
    fn index(&self, index: EdgeIndex) -> &Self::Output {
        &self.edges[index.0]
    }
}

impl IndexMut<EdgeIndex> for Graph {
    fn index_mut(&mut self, index: EdgeIndex) -> &mut Self::Output {
        &mut self.edges[index.0]
    }
}

#[derive(Debug)]
pub struct GraphModel {
    pub graph: Graph,
    pub node_map: HashMap<NodeId, NodeIndex>,
    pub root: Option<NodeId>,
}

impl Default for GraphModel {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphModel {
    pub fn new() -> Self {
        Self {
            graph: Graph::new(),
            node_map: HashMap::new(),
            root: None,
        }
    }

    pub fn add_node(&mut self, node: Node) {
        if !self.node_map.contains_key(&node.id) {
            let dummy = DummyNode {
                id: node.id,
                node_kind: node.kind,
                name: node.serialized_name,
                position: Vec2::default(),
                size: Vec2::new(100.0, 30.0),
                visible: true,
                active: false,
                focused: false,
                expanded: false,
                children: Vec::new(),
                parent: None,
                bundle_info: None,
                bundled_nodes: Vec::new(),
                group_type: None,
                group_layout: GroupLayout::GRID,
            };
            let idx = self.graph.add_node(dummy);
            self.node_map.insert(node.id, idx);
        }
    }

    pub fn add_edge(&mut self, edge: Edge) {
        if let (Some(&src), Some(&target)) = (
            self.node_map.get(&edge.source),
            self.node_map.get(&edge.target),
        ) {
            let dummy = DummyEdge {
                id: edge.id,
                source: edge.source,
                target: edge.target,
                kind: edge.kind,
                visible: true,
                active: false,
                source_idx: src,
                target_idx: target,
            };
            self.graph.add_edge(src, target, dummy);
        } else {
            // Debug logging for missing nodes
            if !self.node_map.contains_key(&edge.source) {
                tracing::warn!(
                    "Dropping edge {:?} because source node {} is missing from graph model",
                    edge.id,
                    edge.source.0
                );
            }
            if !self.node_map.contains_key(&edge.target) {
                tracing::warn!(
                    "Dropping edge {:?} because target node {} is missing from graph model",
                    edge.id,
                    edge.target.0
                );
            }
        }
    }

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    pub fn get_node(&self, id: NodeId) -> Option<&DummyNode> {
        self.node_map.get(&id).map(|&idx| &self.graph[idx])
    }

    pub fn get_node_mut(&mut self, id: NodeId) -> Option<&mut DummyNode> {
        self.node_map.get(&id).map(|&idx| &mut self.graph[idx])
    }

    pub fn rebuild_hierarchy(&mut self) {
        // Clear existing hierarchy
        let indices: Vec<_> = self.graph.node_indices().collect();
        for node_idx in indices {
            let node = &mut self.graph[node_idx];
            node.children.clear();
            node.parent = None;
        }

        let mut parent_child_pairs = Vec::new();
        for edge_idx in self.graph.edge_indices() {
            let edge = &self.graph[edge_idx];
            if edge.kind == codestory_core::EdgeKind::MEMBER {
                parent_child_pairs.push((edge.source, edge.target));
            }
        }

        for (parent_id, child_id) in parent_child_pairs {
            if let (Some(&p_idx), Some(&c_idx)) =
                (self.node_map.get(&parent_id), self.node_map.get(&child_id))
            {
                // Check if already has parent to avoid cycles/multi-parents in tree
                if self.graph[c_idx].parent.is_none() {
                    self.graph[c_idx].parent = Some(parent_id);
                    self.graph[p_idx].children.push(child_id);
                }
            }
        }
    }

    pub fn expand_all(&mut self) {
        let indices: Vec<_> = self.graph.node_indices().collect();
        for node_idx in indices {
            self.graph[node_idx].expanded = true;
        }
    }

    pub fn collapse_all(&mut self) {
        let indices: Vec<_> = self.graph.node_indices().collect();
        for node_idx in indices {
            self.graph[node_idx].expanded = false;
        }
    }

    /// Extract nodes and edges after bundling/hierarchy changes
    /// Note: This preserves bundle_info in DummyNode for later use
    pub fn get_dummy_data(&self) -> (Vec<DummyNode>, Vec<DummyEdge>) {
        let nodes = self.graph.nodes.clone();
        let edges = self.graph.edges.clone();
        (nodes, edges)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_core::{Edge, EdgeKind, NodeId, NodeKind};

    #[test]
    fn test_graph_model() {
        let mut model = GraphModel::new();
        let n1 = Node {
            id: NodeId(1),
            kind: NodeKind::CLASS,
            serialized_name: "C1".to_string(),
            ..Default::default()
        };
        let n2 = Node {
            id: NodeId(2),
            kind: NodeKind::METHOD,
            serialized_name: "M1".to_string(),
            ..Default::default()
        };

        model.add_node(n1);
        model.add_node(n2);

        let e1 = Edge {
            id: codestory_core::EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::MEMBER,
            ..Default::default()
        };
        model.add_edge(e1);

        assert_eq!(model.node_count(), 2);
        assert_eq!(model.edge_count(), 1);
    }
}
