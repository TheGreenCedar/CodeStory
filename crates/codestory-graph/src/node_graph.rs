use codestory_core::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PinType {
    Standard,
    Inheritance,
    Composition,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeMember {
    pub id: NodeId,
    pub name: String,
    pub kind: codestory_core::NodeKind,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeGraphNode {
    pub id: NodeId,
    pub parent_id: Option<NodeId>,
    pub kind: codestory_core::NodeKind,
    pub label: String,
    pub members: Vec<NodeMember>,
    pub inputs: Vec<NodeGraphPin>,
    pub outputs: Vec<NodeGraphPin>,
    pub bundle_info: Option<codestory_core::BundleInfo>,
    /// Whether this node is indexed (affects hatching pattern overlay)
    /// Non-indexed nodes (external/unresolved symbols) will have diagonal hatching
    #[serde(default = "default_is_indexed")]
    pub is_indexed: bool,
}

/// Default value for is_indexed field (true for backwards compatibility)
fn default_is_indexed() -> bool {
    true
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeGraphPin {
    pub label: String,
    pub pin_type: PinType,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeGraphEdge {
    pub id: codestory_core::EdgeId,
    pub source_node: NodeId,
    pub source_output_index: usize,
    pub target_node: NodeId,
    pub target_input_index: usize,
    pub edge_type: PinType,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeGraph {
    pub nodes: Vec<NodeGraphNode>,
    pub edges: Vec<NodeGraphEdge>,
}
