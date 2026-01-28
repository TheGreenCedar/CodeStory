use crate::graph::{DummyNode, GraphModel, GroupLayout, Vec2};
use codestory_core::{BundleInfo, NodeId, NodeKind};
use std::collections::HashMap;

pub struct NodeBundler {
    pub threshold: usize,
}

impl NodeBundler {
    pub fn new(threshold: usize) -> Self {
        Self { threshold }
    }

    pub fn execute(&self, model: &mut GraphModel) {
        let mut bundles_to_create = Vec::new();
        let mut next_bundle_id = -1000;

        // 1. Identify bundles
        for node_idx in model.graph.node_indices() {
            let node = &model.graph[node_idx];

            // Only bundle children of expanded nodes or just structural children?
            // Usually we bundle regardless of expansion, but visuals hide it.
            // But here we are modifying the structure.

            if node.children.is_empty() {
                continue;
            }

            let mut groups: HashMap<NodeKind, Vec<NodeId>> = HashMap::new();
            for &child_id in &node.children {
                if let Some(child_node) = model.get_node(child_id) {
                    // Bundle all types if they happen in large groups within a parent
                    // This matches Sourcetrail's behavior for "Classes", "Structs", etc.
                    if !child_node
                        .bundle_info
                        .as_ref()
                        .map(|b| b.is_bundle)
                        .unwrap_or(false)
                    {
                        groups
                            .entry(child_node.node_kind)
                            .or_default()
                            .push(child_id);
                    }
                }
            }

            for (kind, children) in groups {
                if children.len() > self.threshold {
                    bundles_to_create.push((node.id, kind, children, next_bundle_id));
                    next_bundle_id -= 1;
                }
            }
        }

        // 2. Apply mutations
        for (parent_id, kind, children_ids, bundle_id_raw) in bundles_to_create {
            let bundle_id = NodeId(bundle_id_raw);
            let bundle_name = format!("{}s", format!("{:?}", kind).to_lowercase()); // e.g. "methods"

            // Create bundle node
            let bundle_node = DummyNode {
                id: bundle_id,
                node_kind: kind,
                name: bundle_name,
                position: Vec2::default(),
                size: Vec2::new(150.0, 30.0), // Initial size
                visible: true,
                active: false,
                focused: false,
                expanded: false, // Collapsed by default
                children: children_ids.clone(),
                parent: Some(parent_id),
                bundle_info: Some(BundleInfo {
                    is_bundle: true,
                    bundle_id: Some(bundle_id_raw),
                    layout_vertical: true,
                    connected_count: children_ids.len(),
                }),
                bundled_nodes: children_ids.clone(),
                group_type: None,
                group_layout: GroupLayout::LIST,
            };

            // Add to graph
            let bundle_idx = model.graph.add_node(bundle_node);
            model.node_map.insert(bundle_id, bundle_idx);

            // Update parent: Remove individual children, add bundle
            if let Some(parent_idx) = model.node_map.get(&parent_id) {
                // Remove bundled children from parent's children list
                // (We keep them in the graph, but re-parent them effectively)
                let parent = &mut model.graph[*parent_idx];
                parent.children.retain(|c| !children_ids.contains(c));
                parent.children.push(bundle_id);
            }

            // Update children: Set parent to bundle
            for child_id in children_ids {
                if let Some(child_node) = model.get_node_mut(child_id) {
                    child_node.parent = Some(bundle_id);
                }
            }
        }
    }
}
