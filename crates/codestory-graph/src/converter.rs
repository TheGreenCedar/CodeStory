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

    pub fn convert_dummies(&self, nodes: &[DummyNode], edges: &[DummyEdge]) -> NodeGraph {
        let mut graph_nodes: HashMap<NodeId, NodeGraphNode> = HashMap::new();
        let mut graph_edges = Vec::new();
        let mut node_map: HashMap<NodeId, &DummyNode> = HashMap::new();
        let mut member_to_host: HashMap<NodeId, NodeId> = HashMap::new();

        for node in nodes {
            node_map.insert(node.id, node);
        }

        // 1. Create Structural Nodes
        for node in nodes {
            let is_bundle = node
                .bundle_info
                .as_ref()
                .map(|b| b.is_bundle)
                .unwrap_or(false);

            // If it's a bundle of members (e.g. methods), skip creating a node for it.
            if is_bundle && !Self::is_structural(node.node_kind) {
                continue;
            }
            // If it's a raw member node (unbundled), skip creating a node for it.
            if !is_bundle && !Self::is_structural(node.node_kind) && node.parent.is_some() {
                continue;
            }

            // Create the node
            let inputs = vec![NodeGraphPin {
                label: "References".to_string(),
                pin_type: PinType::Standard,
            }];

            let mut outputs = vec![NodeGraphPin {
                label: "Calls".to_string(),
                pin_type: PinType::Standard,
            }];

            if Self::is_structural(node.node_kind) {
                outputs.push(NodeGraphPin {
                    label: "Inherited By".to_string(),
                    pin_type: PinType::Inheritance,
                });
            }

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
        for node in nodes {
            let is_bundle = node
                .bundle_info
                .as_ref()
                .map(|b| b.is_bundle)
                .unwrap_or(false);
            let is_structural = Self::is_structural(node.node_kind);

            if !is_structural {
                if is_bundle {
                    // It's a bundle of members. Children are members. Parent is host.
                    if let Some(parent_id) = node.parent
                        && let Some(host) = graph_nodes.get_mut(&parent_id)
                    {
                        for &child_id in &node.children {
                            if let Some(child) = node_map.get(&child_id) {
                                host.members.push(NodeMember {
                                    id: child.id,
                                    name: child.name.clone(),
                                    kind: child.node_kind,
                                });
                                member_to_host.insert(child_id, parent_id);
                            }
                        }
                    }
                } else if let Some(parent_id) = node.parent {
                    // It's an individual member node
                    // Check if parent is a bundle. If so, it was handled above (via bundle iteration).
                    let parent_is_bundle = node_map
                        .get(&parent_id)
                        .map(|n| n.bundle_info.as_ref().map(|b| b.is_bundle).unwrap_or(false))
                        .unwrap_or(false);

                    if !parent_is_bundle && let Some(host) = graph_nodes.get_mut(&parent_id) {
                        host.members.push(NodeMember {
                            id: node.id,
                            name: node.name.clone(),
                            kind: node.node_kind,
                        });
                        member_to_host.insert(node.id, parent_id);
                    }
                }
            }
        }

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

            let (pin_type, source_output_index) = match edge.kind {
                EdgeKind::INHERITANCE | EdgeKind::OVERRIDE => (PinType::Inheritance, 1),
                EdgeKind::CALL => (PinType::Standard, 0),
                EdgeKind::USAGE | EdgeKind::TYPE_USAGE => (PinType::Standard, 0),
                EdgeKind::IMPORT | EdgeKind::INCLUDE => (PinType::Standard, 0),
                _ => (PinType::Standard, 0),
            };

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
        let mut node_map: HashMap<NodeId, &DummyNode> = HashMap::new();
        let mut member_to_host: HashMap<NodeId, NodeId> = HashMap::new();

        // Build node map for quick lookup
        for node in nodes {
            node_map.insert(node.id, node);
        }

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
            let is_bundle = node
                .bundle_info
                .as_ref()
                .map(|b| b.is_bundle)
                .unwrap_or(false);

            // Skip bundled non-structural nodes (they become members)
            if is_bundle && !Self::is_structural(node.node_kind) {
                continue;
            }
            // Skip unbundled member nodes with parents (they become members)
            if !is_bundle && !Self::is_structural(node.node_kind) && node.parent.is_some() {
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
            let inputs = vec![NodeGraphPin {
                label: "References".to_string(),
                pin_type: PinType::Standard,
            }];

            let mut outputs = vec![NodeGraphPin {
                label: "Calls".to_string(),
                pin_type: PinType::Standard,
            }];

            if Self::is_structural(node.node_kind) {
                outputs.push(NodeGraphPin {
                    label: "Inherited By".to_string(),
                    pin_type: PinType::Inheritance,
                });
            }

            pin_info.insert(node.id, (inputs, outputs));
        }

        // 2. Group Members into Visibility Sections
        for node in nodes {
            let is_bundle = node
                .bundle_info
                .as_ref()
                .map(|b| b.is_bundle)
                .unwrap_or(false);
            let is_structural = Self::is_structural(node.node_kind);

            if !is_structural {
                if is_bundle {
                    // It's a bundle of members. Children are members. Parent is host.
                    if let Some(parent_id) = node.parent
                        && let Some(host) = uml_nodes.get_mut(&parent_id)
                    {
                        let mut functions = Vec::new();
                        let mut variables = Vec::new();
                        let mut other = Vec::new();

                        for &child_id in &node.children {
                            if let Some(child) = node_map.get(&child_id) {
                                let mut member =
                                    MemberItem::new(child.id, child.node_kind, child.name.clone());

                                // Set whether this member has outgoing edges
                                member.set_has_outgoing_edges(
                                    members_with_outgoing_edges.contains(&child.id),
                                );

                                // Group by kind
                                match child.node_kind {
                                    NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO => {
                                        functions.push(member);
                                    }
                                    NodeKind::FIELD
                                    | NodeKind::VARIABLE
                                    | NodeKind::GLOBAL_VARIABLE
                                    | NodeKind::CONSTANT
                                    | NodeKind::ENUM_CONSTANT => {
                                        variables.push(member);
                                    }
                                    _ => {
                                        other.push(member);
                                    }
                                }
                                member_to_host.insert(child_id, parent_id);
                            }
                        }

                        // Add non-empty sections to host
                        if !functions.is_empty() {
                            host.visibility_sections
                                .push(VisibilitySection::with_members(
                                    VisibilityKind::Functions,
                                    functions,
                                ));
                        }
                        if !variables.is_empty() {
                            host.visibility_sections
                                .push(VisibilitySection::with_members(
                                    VisibilityKind::Variables,
                                    variables,
                                ));
                        }
                        if !other.is_empty() {
                            host.visibility_sections
                                .push(VisibilitySection::with_members(
                                    VisibilityKind::Other,
                                    other,
                                ));
                        }
                    }
                } else if let Some(parent_id) = node.parent {
                    // It's an individual member node
                    let parent_is_bundle = node_map
                        .get(&parent_id)
                        .map(|n| n.bundle_info.as_ref().map(|b| b.is_bundle).unwrap_or(false))
                        .unwrap_or(false);

                    if !parent_is_bundle && let Some(host) = uml_nodes.get_mut(&parent_id) {
                        let mut member =
                            MemberItem::new(node.id, node.node_kind, node.name.clone());

                        // Set whether this member has outgoing edges
                        member
                            .set_has_outgoing_edges(members_with_outgoing_edges.contains(&node.id));

                        // Determine which section this member belongs to
                        let visibility = match node.node_kind {
                            NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO => {
                                VisibilityKind::Functions
                            }
                            NodeKind::FIELD
                            | NodeKind::VARIABLE
                            | NodeKind::GLOBAL_VARIABLE
                            | NodeKind::CONSTANT
                            | NodeKind::ENUM_CONSTANT => VisibilityKind::Variables,
                            _ => VisibilityKind::Other,
                        };

                        // Find or create the appropriate section
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

                        member_to_host.insert(node.id, parent_id);
                    }
                }
            }
        }

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

            let (pin_type, source_output_index) = match edge.kind {
                EdgeKind::INHERITANCE | EdgeKind::OVERRIDE => (PinType::Inheritance, 1),
                EdgeKind::CALL => (PinType::Standard, 0),
                EdgeKind::USAGE | EdgeKind::TYPE_USAGE => (PinType::Standard, 0),
                EdgeKind::IMPORT | EdgeKind::INCLUDE => (PinType::Standard, 0),
                _ => (PinType::Standard, 0),
            };

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
