use crate::graph::{DummyEdge, DummyNode};
use crate::node_graph::{
    NodeGraph, NodeGraphEdge, NodeGraphNode, NodeGraphPin, NodeMember, PinType,
};
use crate::uml_types::{MemberItem, UmlNode, VisibilityKind, VisibilitySection};
use codestory_core::{EdgeKind, NodeId, NodeKind};
use std::collections::HashMap;

type NodeGraphPinMap = HashMap<NodeId, (Vec<NodeGraphPin>, Vec<NodeGraphPin>)>;
type UmlConversionResult = (Vec<UmlNode>, Vec<NodeGraphEdge>, NodeGraphPinMap);

pub struct NodeGraphConverter;

impl NodeGraphConverter {
    pub fn new() -> Self {
        Self
    }

    fn is_structural(kind: NodeKind) -> bool {
        matches!(
            kind,
            NodeKind::CLASS
                | NodeKind::STRUCT
                | NodeKind::INTERFACE
                | NodeKind::UNION
                | NodeKind::ENUM
                | NodeKind::NAMESPACE
                | NodeKind::MODULE
        )
    }

    fn is_bundle(node: &DummyNode) -> bool {
        node.bundle_info
            .as_ref()
            .map(|b| b.is_bundle)
            .unwrap_or(false)
    }

    fn default_pins(kind: NodeKind) -> (Vec<NodeGraphPin>, Vec<NodeGraphPin>) {
        let inputs = vec![NodeGraphPin {
            label: "References".to_string(),
            pin_type: PinType::Standard,
        }];

        let mut outputs = vec![NodeGraphPin {
            label: "Calls".to_string(),
            pin_type: PinType::Standard,
        }];

        if Self::is_structural(kind) {
            outputs.push(NodeGraphPin {
                label: "Inherited By".to_string(),
                pin_type: PinType::Inheritance,
            });
        }

        (inputs, outputs)
    }

    fn parent_is_bundle(node_map: &HashMap<NodeId, &DummyNode>, parent_id: NodeId) -> bool {
        node_map
            .get(&parent_id)
            .map(|n| Self::is_bundle(n))
            .unwrap_or(false)
    }

    fn edge_pin(edge_kind: EdgeKind) -> (PinType, usize) {
        match edge_kind {
            EdgeKind::INHERITANCE | EdgeKind::OVERRIDE => (PinType::Inheritance, 1),
            _ => (PinType::Standard, 0),
        }
    }

    fn member_visibility(node_kind: NodeKind) -> VisibilityKind {
        match node_kind {
            NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO => VisibilityKind::Functions,
            NodeKind::FIELD
            | NodeKind::VARIABLE
            | NodeKind::GLOBAL_VARIABLE
            | NodeKind::CONSTANT
            | NodeKind::ENUM_CONSTANT => VisibilityKind::Variables,
            _ => VisibilityKind::Other,
        }
    }

    fn build_node_map(nodes: &[DummyNode]) -> HashMap<NodeId, &DummyNode> {
        nodes.iter().map(|node| (node.id, node)).collect()
    }

    fn should_emit_host_node(node: &DummyNode) -> bool {
        let is_bundle = Self::is_bundle(node);
        let is_structural = Self::is_structural(node.node_kind);
        if is_bundle && !is_structural {
            return false;
        }
        if !is_bundle && !is_structural && node.parent.is_some() {
            return false;
        }
        true
    }

    fn for_each_member<F>(nodes: &[DummyNode], node_map: &HashMap<NodeId, &DummyNode>, mut f: F)
    where
        F: FnMut(NodeId, &DummyNode),
    {
        for node in nodes {
            if Self::is_structural(node.node_kind) {
                continue;
            }
            if Self::is_bundle(node) {
                if let Some(parent_id) = node.parent {
                    for &child_id in &node.children {
                        if let Some(child) = node_map.get(&child_id) {
                            f(parent_id, child);
                        }
                    }
                }
                continue;
            }

            if let Some(parent_id) = node.parent
                && !Self::parent_is_bundle(node_map, parent_id)
            {
                f(parent_id, node);
            }
        }
    }

    pub fn convert_dummies(&self, nodes: &[DummyNode], edges: &[DummyEdge]) -> NodeGraph {
        let mut graph_nodes: HashMap<NodeId, NodeGraphNode> = HashMap::new();
        let mut graph_edges = Vec::new();
        let node_map = Self::build_node_map(nodes);
        let mut member_to_host: HashMap<NodeId, NodeId> = HashMap::new();

        // 1. Create Structural Nodes
        for node in nodes {
            if !Self::should_emit_host_node(node) {
                continue;
            }

            // Create the node
            let (inputs, outputs) = Self::default_pins(node.node_kind);

            graph_nodes.insert(
                node.id,
                NodeGraphNode {
                    id: node.id,
                    parent_id: node.parent,
                    kind: node.node_kind,
                    label: node.name.clone(),
                    members: Vec::new(),
                    inputs,
                    outputs,
                    bundle_info: node.bundle_info.clone(),
                    // TODO: Set is_indexed based on actual indexing status from storage
                    // This requires:
                    // 1. Adding is_indexed BOOLEAN field to storage node table schema
                    // 2. Updating indexing pipeline to set is_indexed=1 when processing files
                    // 3. Updating Node struct in codestory-core to include is_indexed field
                    // 4. Passing through is_indexed from storage queries to this conversion
                    // For now, default to true (all nodes treated as indexed, hatching pattern disabled)
                    is_indexed: true,
                },
            );
        }

        // 2. Process Members (populate members list and member_to_host map)
        Self::for_each_member(nodes, &node_map, |parent_id, member| {
            if let Some(host) = graph_nodes.get_mut(&parent_id) {
                host.members.push(NodeMember {
                    id: member.id,
                    name: member.name.clone(),
                    kind: member.node_kind,
                });
                member_to_host.insert(member.id, parent_id);
            }
        });

        // 3. Convert Edges
        for edge in edges {
            // Resolve endpoints
            let source = *member_to_host.get(&edge.source).unwrap_or(&edge.source);
            let target = *member_to_host.get(&edge.target).unwrap_or(&edge.target);

            // Skip member edges (they are implicit in the list)
            if edge.kind == EdgeKind::MEMBER {
                continue;
            }

            // Check if source/target exist in graph_nodes
            if !graph_nodes.contains_key(&source) || !graph_nodes.contains_key(&target) {
                continue;
            }

            // Reroute self-loops? Or keep them?
            // If Class A calls Class A (internal call), it's a self-loop.
            // Snarl can handle self-loops.

            let (pin_type, source_output_index) = Self::edge_pin(edge.kind);

            let source_node = graph_nodes.get(&source).unwrap();
            let valid_index = if source_output_index < source_node.outputs.len() {
                source_output_index
            } else {
                0
            };

            graph_edges.push(NodeGraphEdge {
                id: edge.id,
                source_node: source,
                source_output_index: valid_index,
                target_node: target,
                target_input_index: 0,
                edge_type: pin_type,
            });
        }

        NodeGraph {
            nodes: graph_nodes.into_values().collect(),
            edges: graph_edges,
        }
    }

    /// Convert DummyNode/DummyEdge to UmlNode with pre-grouped visibility sections.
    /// Also returns pin information for the adapter to store separately.
    /// Returns pin information for the adapter to store separately.
    pub fn convert_dummies_to_uml(
        &self,
        nodes: &[DummyNode],
        edges: &[DummyEdge],
    ) -> UmlConversionResult {
        let mut uml_nodes: HashMap<NodeId, UmlNode> = HashMap::new();
        let mut graph_edges = Vec::new();
        let mut pin_info: NodeGraphPinMap = HashMap::new();
        let node_map = Self::build_node_map(nodes);
        let mut member_to_host: HashMap<NodeId, NodeId> = HashMap::new();

        // Build set of member IDs that have outgoing edges
        // A member has outgoing edges if it appears as the source of any edge
        let mut members_with_outgoing_edges = std::collections::HashSet::new();
        for edge in edges {
            // Check if the edge source is a member (non-structural node)
            if let Some(source_node) = node_map.get(&edge.source)
                && !Self::is_structural(source_node.node_kind)
            {
                members_with_outgoing_edges.insert(edge.source);
            }
        }

        // 1. Create Structural Nodes (classes, structs, etc.)
        for node in nodes {
            if !Self::should_emit_host_node(node) {
                continue;
            }

            // Create UmlNode
            let mut uml_node = UmlNode::new(node.id, node.node_kind, node.name.clone());
            uml_node.parent_id = node.parent;
            uml_node.is_indexed = true; // TODO: same as before
            uml_node.bundle_info = node.bundle_info.as_ref().map(|bi| {
                crate::uml_types::BundleInfo {
                    bundled_node_ids: Vec::new(), // TODO: populate if needed
                    count: bi.connected_count,
                    is_expanded: false,
                }
            });

            uml_nodes.insert(node.id, uml_node);

            // Compute pins for this node
            let (inputs, outputs) = Self::default_pins(node.node_kind);

            pin_info.insert(node.id, (inputs, outputs));
        }

        // 2. Group Members into Visibility Sections
        Self::for_each_member(nodes, &node_map, |parent_id, member_node| {
            if let Some(host) = uml_nodes.get_mut(&parent_id) {
                let mut member = MemberItem::new(
                    member_node.id,
                    member_node.node_kind,
                    member_node.name.clone(),
                );
                member
                    .set_has_outgoing_edges(members_with_outgoing_edges.contains(&member_node.id));

                let visibility = Self::member_visibility(member_node.node_kind);
                if let Some(section) = host
                    .visibility_sections
                    .iter_mut()
                    .find(|s| s.kind == visibility)
                {
                    section.members.push(member);
                } else {
                    host.visibility_sections
                        .push(VisibilitySection::with_members(visibility, vec![member]));
                }
                member_to_host.insert(member_node.id, parent_id);
            }
        });

        // 3. Convert Edges (same logic as before)
        for edge in edges {
            // Resolve endpoints
            let source = *member_to_host.get(&edge.source).unwrap_or(&edge.source);
            let target = *member_to_host.get(&edge.target).unwrap_or(&edge.target);

            // Skip member edges
            if edge.kind == EdgeKind::MEMBER {
                continue;
            }

            // Check if source/target exist
            if !uml_nodes.contains_key(&source) || !uml_nodes.contains_key(&target) {
                continue;
            }

            let (pin_type, source_output_index) = Self::edge_pin(edge.kind);

            // Validate pin index
            let valid_index = if let Some((_, outputs)) = pin_info.get(&source) {
                if source_output_index < outputs.len() {
                    source_output_index
                } else {
                    0
                }
            } else {
                0
            };

            graph_edges.push(NodeGraphEdge {
                id: edge.id,
                source_node: source,
                source_output_index: valid_index,
                target_node: target,
                target_input_index: 0,
                edge_type: pin_type,
            });
        }

        (uml_nodes.into_values().collect(), graph_edges, pin_info)
    }
}

impl Default for NodeGraphConverter {
    fn default() -> Self {
        Self::new()
    }
}
