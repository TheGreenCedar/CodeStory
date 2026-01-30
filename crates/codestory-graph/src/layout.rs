use crate::graph::{EdgeIndex, GraphModel, GroupLayout, NodeIndex, Vec2};
use codestory_core::{LayoutDirection, NodeId};
use std::collections::HashMap;

/// Helper to compute topological ranks for nodes.
/// Returns a map of NodeIndex -> Rank (0-based level).
/// Handles cycles by limiting iterations.
pub fn compute_dag_ranks(model: &GraphModel) -> HashMap<NodeIndex, i32> {
    let mut ranks: HashMap<NodeIndex, i32> = HashMap::new();
    for node in model.graph.node_indices() {
        ranks.insert(node, 0);
    }

    let max_iterations = (model.node_count() + 2).min(1000);

    for _ in 0..max_iterations {
        let mut changed = false;
        for edge in model.graph.edge_indices() {
            if let Some((source, target)) = model.graph.edge_endpoints(edge) {
                if source == target {
                    continue;
                }

                // Only consider "structural" edges for ranking if we wanted to be selective,
                // but for general flow, Call/Usage/Inheritance are good.
                // For now, use all edges to define flow.

                let source_rank = *ranks.get(&source).unwrap();
                let target_rank = *ranks.get(&target).unwrap();

                if target_rank <= source_rank {
                    ranks.insert(target, source_rank + 1);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    ranks
}

/// Post-process positions to enforce deterministic orientation
/// flips the graph so the lexicographically first node is top-left of the last node
fn canonicalize_positions(positions: &mut HashMap<NodeIndex, (f32, f32)>, model: &GraphModel) {
    if positions.len() < 2 {
        return;
    }

    // Identify two stable anchor nodes
    let indices: Vec<NodeIndex> = positions.keys().cloned().collect();

    let min_node = indices
        .iter()
        .min_by(|a, b| model.graph[**a].name.cmp(&model.graph[**b].name))
        .unwrap();
    let max_node = indices
        .iter()
        .max_by(|a, b| model.graph[**a].name.cmp(&model.graph[**b].name))
        .unwrap();

    if min_node == max_node {
        return;
    }

    let p1 = positions[min_node];
    let p2 = positions[max_node];

    // Horizontal check: Ensure min_node is to the left of max_node
    if p1.0 > p2.0 {
        for pos in positions.values_mut() {
            pos.0 = -pos.0;
        }
    }

    // Vertical check: Ensure min_node is above max_node
    // (We re-fetch positions because they might have been flipped horizontally)
    let p1 = positions[min_node];
    let p2 = positions[max_node];

    if p1.1 > p2.1 {
        for pos in positions.values_mut() {
            pos.1 = -pos.1;
        }
    }
}

pub trait Layouter {
    fn execute(&self, model: &GraphModel) -> HashMap<NodeIndex, (f32, f32)>;
}

pub trait EnhancedLayouter {
    fn execute_enhanced(
        &self,
        model: &GraphModel,
    ) -> (HashMap<NodeIndex, (f32, f32)>, HashMap<NodeIndex, Vec2>);
}

pub struct Bucket {
    pub nodes: Vec<NodeIndex>,
    pub width: f32,
    pub height: f32,
}

impl Default for Bucket {
    fn default() -> Self {
        Self::new()
    }
}

impl Bucket {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            width: 0.0,
            height: 0.0,
        }
    }

    pub fn add_node(&mut self, node_idx: NodeIndex, size: Vec2) {
        self.nodes.push(node_idx);
        self.width = self.width.max(size.x);
        self.height += size.y + 10.0; // Spacing
    }
}

pub struct BucketLayouter {
    pub node_sizes: HashMap<NodeId, Vec2>,
}

impl Layouter for BucketLayouter {
    fn execute(&self, model: &GraphModel) -> HashMap<NodeIndex, (f32, f32)> {
        let mut positions = HashMap::new();
        if model.node_count() == 0 {
            return positions;
        }

        let node_count = model.node_count();
        let cols = (node_count as f32).sqrt().ceil() as usize;

        let mut x = 0.0;
        let mut y = 0.0;
        let mut max_row_height = 0.0;

        for (i, node_idx) in model.graph.node_indices().enumerate() {
            let size = self
                .node_sizes
                .get(&model.graph[node_idx].id)
                .cloned()
                .unwrap_or(Vec2::new(100.0, 30.0));

            if i > 0 && i % cols == 0 {
                x = 0.0;
                y += max_row_height + 50.0;
                max_row_height = 0.0;
            }

            positions.insert(node_idx, (x, y));

            x += size.x + 50.0;
            max_row_height = max_row_height.max(size.y);
        }

        positions
    }
}

pub struct ListLayouter {
    pub spacing: f32,
}

impl Layouter for ListLayouter {
    fn execute(&self, model: &GraphModel) -> HashMap<NodeIndex, (f32, f32)> {
        let mut positions = HashMap::new();
        let mut y = 0.0;
        for node_idx in model.graph.node_indices() {
            positions.insert(node_idx, (0.0, y));
            y += self.spacing;
        }
        positions
    }
}

pub struct GridLayouter {
    pub spacing: f32,
}

impl Layouter for GridLayouter {
    fn execute(&self, model: &GraphModel) -> HashMap<NodeIndex, (f32, f32)> {
        let mut positions = HashMap::new();
        let n = model.node_count();
        if n == 0 {
            return positions;
        }

        let cols = (n as f32).sqrt().ceil() as usize;

        // Sort nodes by Scope (Namespace), then Kind, then Name
        // Serialized ID format is generally "n:scope.c:Class" etc.
        // We'll roughly parse it to find the "Package/Crate" prefix.
        let mut sorted_indices: Vec<NodeIndex> = model.graph.node_indices().collect();
        sorted_indices.sort_by(|&a, &b| {
            let node_a = &model.graph[a];
            let node_b = &model.graph[b];

            // Simple heuristic: split by first '.' to get top-level scope
            let scope_a = node_a.name.split('.').next().unwrap_or("");
            let scope_b = node_b.name.split('.').next().unwrap_or("");

            scope_a
                .cmp(scope_b)
                .then_with(|| {
                    format!("{:?}", node_a.node_kind).cmp(&format!("{:?}", node_b.node_kind))
                }) // Group by type
                .then_with(|| node_a.name.cmp(&node_b.name))
        });

        for (i, &node_idx) in sorted_indices.iter().enumerate() {
            let x = (i % cols) as f32 * self.spacing;
            let y = (i / cols) as f32 * self.spacing;
            positions.insert(node_idx, (x, y));
        }

        positions
    }
}

/// Radial Layout - places central node at center, others in concentric rings
pub struct RadialLayouter {
    pub ring_spacing: f32,
    pub node_spacing: f32,
}

impl Default for RadialLayouter {
    fn default() -> Self {
        Self {
            ring_spacing: 300.0, // Increased from 150.0
            node_spacing: 150.0, // Increased from 80.0
        }
    }
}

impl Layouter for RadialLayouter {
    fn execute(&self, model: &GraphModel) -> HashMap<NodeIndex, (f32, f32)> {
        let mut positions = HashMap::new();
        if model.node_count() == 0 {
            return positions;
        }

        // 1. Convert to oak_visualize Graph - Deterministic Order
        let mut oak_graph = Graph::new(true);
        let mut id_map: HashMap<NodeIndex, String> = HashMap::new();

        // Sort nodes by name
        let mut sorted_nodes: Vec<NodeIndex> = model.graph.node_indices().collect();
        sorted_nodes.sort_by(|a, b| model.graph[*a].name.cmp(&model.graph[*b].name));

        for node_idx in sorted_nodes {
            let node = &model.graph[node_idx];
            let id_str = node_idx.to_string();
            oak_graph.add_node(GraphNode {
                id: id_str.clone(),
                label: node.name.clone(),
                node_type: "default".to_string(),
                size: Some(Size {
                    width: node.size.x as f64,
                    height: node.size.y as f64,
                }),
                attributes: HashMap::new(),
                weight: 1.0,
            });
            id_map.insert(node_idx, id_str);
        }

        // Sort edges
        let mut sorted_edges: Vec<EdgeIndex> = model.graph.edge_indices().collect();
        sorted_edges.sort_by(|a, b| {
            let (sa, ta) = model.graph.edge_endpoints(*a).unwrap();
            let (sb, tb) = model.graph.edge_endpoints(*b).unwrap();
            let sa_name = &model.graph[sa].name;
            let ta_name = &model.graph[ta].name;
            let sb_name = &model.graph[sb].name;
            let tb_name = &model.graph[tb].name;
            (sa_name, ta_name).cmp(&(sb_name, tb_name))
        });

        for edge_idx in sorted_edges {
            if let Some((source, target)) = model.graph.edge_endpoints(edge_idx)
                && let (Some(s), Some(t)) = (id_map.get(&source), id_map.get(&target))
            {
                oak_graph.add_edge(GraphEdge::new(s.clone(), t.clone()));
            }
        }

        // 2. Configure Layout
        let config = GraphLayoutConfig {
            node_spacing: self.node_spacing as f64,
            circle_radius: self.ring_spacing as f64,
            ..Default::default()
        };

        // 3. Execute
        let layout = GraphLayout::circular().with_config(config);
        match layout.layout_graph(&oak_graph) {
            Ok(result) => {
                for (node_idx, id_str) in id_map {
                    if let Some(pos) = result.nodes.get(&id_str) {
                        positions.insert(
                            node_idx,
                            (pos.rect.origin.x as f32, pos.rect.origin.y as f32),
                        );
                    }
                }
            }
            Err(e) => {
                tracing::error!("oak-visualize radial layout failed: {:?}", e);
            }
        }

        canonicalize_positions(&mut positions, model);

        positions
    }
}

/// Force-Directed Layout - uses spring physics for natural node distribution
pub struct ForceDirectedLayouter {
    pub iterations: usize,
    pub ideal_edge_length: f32,
    pub repulsion_strength: f32,
    pub attraction_strength: f32,
    pub rank_strength: f32, // New: Strength of the force pulling nodes to their rank Y-level
}

impl Default for ForceDirectedLayouter {
    fn default() -> Self {
        Self {
            iterations: 300,
            ideal_edge_length: 250.0,
            repulsion_strength: 20000.0,
            attraction_strength: 0.05,
            rank_strength: 0.1, // Gentle vertical guidance
        }
    }
}

use oak_visualize::geometry::Size;
use oak_visualize::graph::{Graph, GraphEdge, GraphLayout, GraphLayoutConfig, GraphNode};

impl Layouter for ForceDirectedLayouter {
    fn execute(&self, model: &GraphModel) -> HashMap<NodeIndex, (f32, f32)> {
        let mut positions = HashMap::new();
        if model.node_count() == 0 {
            return positions;
        }

        // 1. Convert to oak_visualize Graph - Deterministic Order
        let mut oak_graph = Graph::new(true); // Directed
        let mut id_map: HashMap<NodeIndex, String> = HashMap::new();

        // Sort nodes by name
        let mut sorted_nodes: Vec<NodeIndex> = model.graph.node_indices().collect();
        sorted_nodes.sort_by(|a, b| model.graph[*a].name.cmp(&model.graph[*b].name));

        for node_idx in sorted_nodes {
            let node = &model.graph[node_idx];
            let id_str = node_idx.to_string();

            oak_graph.add_node(GraphNode {
                id: id_str.clone(),
                label: node.name.clone(),
                node_type: "default".to_string(),
                size: Some(Size {
                    width: node.size.x as f64,
                    height: node.size.y as f64,
                }),
                attributes: HashMap::new(),
                weight: 1.0,
            });
            id_map.insert(node_idx, id_str);
        }

        // Sort edges
        let mut sorted_edges: Vec<EdgeIndex> = model.graph.edge_indices().collect();
        sorted_edges.sort_by(|a, b| {
            let (sa, ta) = model.graph.edge_endpoints(*a).unwrap();
            let (sb, tb) = model.graph.edge_endpoints(*b).unwrap();
            let sa_name = &model.graph[sa].name;
            let ta_name = &model.graph[ta].name;
            let sb_name = &model.graph[sb].name;
            let tb_name = &model.graph[tb].name;
            (sa_name, ta_name).cmp(&(sb_name, tb_name))
        });

        for edge_idx in sorted_edges {
            if let Some((source, target)) = model.graph.edge_endpoints(edge_idx)
                && let (Some(s), Some(t)) = (id_map.get(&source), id_map.get(&target))
            {
                oak_graph.add_edge(GraphEdge::new(s.clone(), t.clone()));
            }
        }

        // 2. Configure & Execute Layout using Builder Pattern
        let layout = GraphLayout::force_directed()
            .with_repulsion(self.repulsion_strength as f64)
            .with_attraction(self.attraction_strength as f64)
            .with_iterations(self.iterations);

        // Assuming layout_graph is the method (based on previous error hint)
        // and it likely takes the graph reference.
        match layout.layout_graph(&oak_graph) {
            Ok(result) => {
                // Map results back
                // result.node_positions is likely HashMap<String, Point>
                for (node_idx, id_str) in id_map {
                    if let Some(pos) = result.nodes.get(&id_str) {
                        positions.insert(
                            node_idx,
                            (pos.rect.origin.x as f32, pos.rect.origin.y as f32),
                        );
                    }
                }
            }
            Err(e) => {
                tracing::error!("oak-visualize force layout failed: {:?}", e);
            }
        }

        canonicalize_positions(&mut positions, model);

        positions
    }
}

pub struct TrailLayouter {
    pub node_sizes: HashMap<NodeId, Vec2>,
    pub layer_spacing: f32,
    pub node_spacing: f32,
    pub direction: LayoutDirection,
}

impl TrailLayouter {
    /// Maximum iterations for ranking convergence
    const MAX_RANKING_ITERATIONS: usize = 1000;

    pub fn new(node_sizes: HashMap<NodeId, Vec2>, direction: LayoutDirection) -> Self {
        Self {
            node_sizes,
            layer_spacing: 120.0,
            node_spacing: 50.0,
            direction,
        }
    }
}

impl Layouter for TrailLayouter {
    fn execute(&self, model: &GraphModel) -> HashMap<NodeIndex, (f32, f32)> {
        let mut positions = HashMap::new();
        if model.node_count() == 0 {
            return positions;
        }

        // 1. Assign Layers (Rank) - Iterative Longest Path
        // We iterate to push nodes down based on dependencies
        let mut ranks: HashMap<NodeIndex, i32> = HashMap::new();

        for node in model.graph.node_indices() {
            ranks.insert(node, 0);
        }

        // Cap iterations to prevent infinite loops in pathological cases (cyclic graphs)
        let max_iterations = (model.node_count() + 2).min(Self::MAX_RANKING_ITERATIONS);
        let mut converged = false;
        for _iteration in 0..max_iterations {
            let mut changed = false;
            for edge in model.graph.edge_indices() {
                if let Some((source, target)) = model.graph.edge_endpoints(edge) {
                    if source == target {
                        continue;
                    } // Self-loop
                    let source_rank = *ranks.get(&source).unwrap();
                    let target_rank = *ranks.get(&target).unwrap();

                    if target_rank < source_rank + 1 {
                        ranks.insert(target, source_rank + 1);
                        changed = true;
                    }
                }
            }
            if !changed {
                converged = true;
                break;
            }
        }

        if !converged {
            tracing::warn!(
                "Layout ranking did not converge after {} iterations (graph may have cycles)",
                max_iterations
            );
        }

        // Group by layer
        let mut layers: HashMap<i32, Vec<NodeIndex>> = HashMap::new();
        for (node, rank) in ranks {
            layers.entry(rank).or_default().push(node);
        }

        // 2. Ordering within layers
        // Simple heuristic: keep order stable or sort by ID/Name
        // A better approach swaps nodes to minimize crossings.
        // For MVP+, we just sort by name for deterministic results.
        for nodes in layers.values_mut() {
            nodes.sort_by_key(|&n| &model.graph[n].name);
        }

        // 3. Assignment
        let sorted_ranks: Vec<_> = {
            let mut k: Vec<_> = layers.keys().cloned().collect();
            k.sort();
            k
        };

        for rank in sorted_ranks {
            if let Some(nodes) = layers.get(&rank) {
                let mut current_offset = 0.0;

                for &node in nodes {
                    let size = self
                        .node_sizes
                        .get(&model.graph[node].id)
                        .cloned()
                        .unwrap_or(Vec2::new(100.0, 30.0));

                    match self.direction {
                        LayoutDirection::Vertical => {
                            positions
                                .insert(node, (current_offset, rank as f32 * self.layer_spacing));
                            current_offset += size.x + self.node_spacing;
                        }
                        LayoutDirection::Horizontal => {
                            positions
                                .insert(node, (rank as f32 * self.layer_spacing, current_offset));
                            current_offset += size.y + self.node_spacing;
                        }
                    }
                }
            }
        }

        positions
    }
}

/// Rank assignment algorithm for hierarchical layout
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RankAlgorithm {
    /// Longest path algorithm - simple and fast
    LongestPath,
}

/// Dagre-style hierarchical layout algorithm.
///
/// Implements a layered graph drawing approach:
/// 1. Rank assignment (longest path) ensuring edges flow left-to-right
/// 2. Crossing minimization via barycenter heuristic with up/down sweeps
/// 3. Coordinate assignment with configurable layer and node spacing
///
/// Supports both horizontal (left-to-right) and vertical (top-to-bottom) orientations.
pub struct HierarchicalLayouter {
    /// Direction of layout flow
    pub direction: LayoutDirection,
    /// Spacing between layers (horizontal distance in LR layout, vertical in TB)
    pub layer_spacing: f32,
    /// Spacing between nodes in the same layer
    pub node_spacing: f32,
    /// Rank assignment algorithm
    pub rank_algorithm: RankAlgorithm,
}

impl Default for HierarchicalLayouter {
    fn default() -> Self {
        Self {
            direction: LayoutDirection::Horizontal,
            layer_spacing: 150.0,
            node_spacing: 50.0,
            rank_algorithm: RankAlgorithm::LongestPath,
        }
    }
}

impl HierarchicalLayouter {
    /// Maximum iterations for rank convergence (handles cycles)
    const MAX_RANKING_ITERATIONS: usize = 1000;
    /// Number of barycenter sweep passes
    const BARYCENTER_PASSES: usize = 4;

    /// Assign ranks to nodes using the longest-path algorithm.
    ///
    /// For any directed edge (A -> B), rank(B) > rank(A).
    /// Nodes with zero incoming edges get rank 0 (leftmost layer).
    pub fn assign_ranks(&self, model: &GraphModel) -> HashMap<NodeIndex, i32> {
        let mut ranks: HashMap<NodeIndex, i32> = HashMap::new();

        // Initialize all nodes to rank 0
        for node in model.graph.node_indices() {
            ranks.insert(node, 0);
        }

        if model.node_count() == 0 {
            return ranks;
        }

        // Iterative longest-path: push targets to rank >= source_rank + 1
        let max_iterations = (model.node_count() + 2).min(Self::MAX_RANKING_ITERATIONS);
        for _ in 0..max_iterations {
            let mut changed = false;
            for edge_idx in model.graph.edge_indices() {
                if let Some((source, target)) = model.graph.edge_endpoints(edge_idx) {
                    if source == target {
                        continue; // skip self-loops
                    }
                    let source_rank = *ranks.get(&source).unwrap();
                    let target_rank = *ranks.get(&target).unwrap();

                    if target_rank < source_rank + 1 {
                        ranks.insert(target, source_rank + 1);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        ranks
    }

    /// Group nodes into layers based on their ranks.
    fn build_layers(&self, ranks: &HashMap<NodeIndex, i32>) -> Vec<Vec<NodeIndex>> {
        let mut layer_map: HashMap<i32, Vec<NodeIndex>> = HashMap::new();
        for (&node, &rank) in ranks {
            layer_map.entry(rank).or_default().push(node);
        }

        let mut sorted_ranks: Vec<i32> = layer_map.keys().cloned().collect();
        sorted_ranks.sort();

        sorted_ranks
            .into_iter()
            .map(|r| {
                let mut layer = layer_map.remove(&r).unwrap();
                // Sort by node index for deterministic initial ordering
                layer.sort();
                layer
            })
            .collect()
    }

    /// Minimize edge crossings using the barycenter heuristic.
    ///
    /// Performs alternating down-sweeps and up-sweeps, reordering nodes
    /// in each layer based on the average position of their neighbors
    /// in the adjacent layer.
    pub fn minimize_crossings(&self, layers: &mut Vec<Vec<NodeIndex>>, model: &GraphModel) {
        if layers.len() <= 1 {
            return;
        }

        // Initial deterministic ordering: sort each layer by node name
        for layer in layers.iter_mut() {
            layer.sort_by_key(|&n| model.graph[n].name.clone());
        }

        // Build position lookup: node -> position within its layer
        let mut positions: HashMap<NodeIndex, f32> = HashMap::new();
        for layer in layers.iter() {
            for (j, &node) in layer.iter().enumerate() {
                positions.insert(node, j as f32);
            }
        }

        for _ in 0..Self::BARYCENTER_PASSES {
            // Down sweep: reorder layer[i] based on neighbors in layer[i-1]
            for i in 1..layers.len() {
                self.reorder_layer_by_barycenter(
                    &mut layers[i],
                    model,
                    &positions,
                    true, // use predecessors (upper layer neighbors)
                );
                // Update positions after reorder
                for (j, &node) in layers[i].iter().enumerate() {
                    positions.insert(node, j as f32);
                }
            }

            // Up sweep: reorder layer[i] based on neighbors in layer[i+1]
            for i in (0..layers.len().saturating_sub(1)).rev() {
                self.reorder_layer_by_barycenter(
                    &mut layers[i],
                    model,
                    &positions,
                    false, // use successors (lower layer neighbors)
                );
                for (j, &node) in layers[i].iter().enumerate() {
                    positions.insert(node, j as f32);
                }
            }
        }
    }

    /// Reorder a single layer by barycenter of neighbors.
    fn reorder_layer_by_barycenter(
        &self,
        layer: &mut [NodeIndex],
        model: &GraphModel,
        positions: &HashMap<NodeIndex, f32>,
        use_predecessors: bool,
    ) {
        let mut barycenters: HashMap<NodeIndex, f32> = HashMap::new();

        for &node in layer.iter() {
            let mut sum = 0.0;
            let mut count = 0;

            for edge_idx in model.graph.edge_indices() {
                if let Some((source, target)) = model.graph.edge_endpoints(edge_idx) {
                    if source == target {
                        continue;
                    }
                    let neighbor = if use_predecessors {
                        // Looking at incoming edges to this node
                        if target == node {
                            Some(source)
                        } else {
                            None
                        }
                    } else {
                        // Looking at outgoing edges from this node
                        if source == node {
                            Some(target)
                        } else {
                            None
                        }
                    };

                    if let Some(n) = neighbor {
                        if let Some(&pos) = positions.get(&n) {
                            sum += pos;
                            count += 1;
                        }
                    }
                }
            }

            let barycenter = if count > 0 {
                sum / count as f32
            } else {
                // Keep current position for nodes without relevant neighbors
                *positions.get(&node).unwrap_or(&0.0)
            };
            barycenters.insert(node, barycenter);
        }

        layer.sort_by(|a, b| {
            barycenters
                .get(a)
                .unwrap_or(&0.0)
                .partial_cmp(barycenters.get(b).unwrap_or(&0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    // Tie-break by name for determinism
                    model.graph[*a].name.cmp(&model.graph[*b].name)
                })
        });
    }

    /// Assign final coordinates to nodes based on layers and ordering.
    ///
    /// For Horizontal direction: layers go left-to-right (x = rank * layer_spacing),
    /// nodes within a layer are stacked vertically (y incremented by node_spacing).
    ///
    /// For Vertical direction: layers go top-to-bottom (y = rank * layer_spacing),
    /// nodes within a layer are spread horizontally (x incremented by node_spacing).
    pub fn assign_coordinates(
        &self,
        layers: &[Vec<NodeIndex>],
        _model: &GraphModel,
    ) -> HashMap<NodeIndex, (f32, f32)> {
        let mut positions: HashMap<NodeIndex, (f32, f32)> = HashMap::new();

        for (depth, layer) in layers.iter().enumerate() {
            let layer_pos = depth as f32 * self.layer_spacing;

            // Center the layer around y=0 (or x=0 for vertical)
            let total_extent =
                (layer.len().saturating_sub(1) as f32) * self.node_spacing;
            let start_offset = -total_extent / 2.0;

            for (j, &node) in layer.iter().enumerate() {
                let node_offset = start_offset + j as f32 * self.node_spacing;

                let pos = match self.direction {
                    LayoutDirection::Horizontal => (layer_pos, node_offset),
                    LayoutDirection::Vertical => (node_offset, layer_pos),
                };

                positions.insert(node, pos);
            }
        }

        positions
    }

    /// Ensure minimum spacing between node bounding boxes.
    /// - Same-layer nodes: at least node_spacing (50px) vertical gap
    /// - Adjacent-layer nodes: at least layer_spacing (150px) horizontal gap
    fn enforce_minimum_spacing(
        &self,
        positions: &mut HashMap<NodeIndex, (f32, f32)>,
        layers: &[Vec<NodeIndex>],
        model: &GraphModel,
    ) {
        // For same-layer spacing, push apart nodes that are too close
        for layer in layers {
            if layer.len() < 2 {
                continue;
            }
            // Sort by current offset in the stacking direction
            let mut ordered: Vec<NodeIndex> = layer.clone();
            ordered.sort_by(|a, b| {
                let pa = positions.get(a).unwrap();
                let pb = positions.get(b).unwrap();
                match self.direction {
                    LayoutDirection::Horizontal => pa.1.partial_cmp(&pb.1).unwrap(),
                    LayoutDirection::Vertical => pa.0.partial_cmp(&pb.0).unwrap(),
                }
            });

            for i in 1..ordered.len() {
                let prev = ordered[i - 1];
                let curr = ordered[i];
                let prev_pos = *positions.get(&prev).unwrap();
                let curr_pos = *positions.get(&curr).unwrap();

                let prev_size = model.graph[prev].size;
                let _curr_size = model.graph[curr].size;

                match self.direction {
                    LayoutDirection::Horizontal => {
                        // Stacking direction is vertical (y)
                        let prev_bottom = prev_pos.1 + prev_size.y;
                        let min_y = prev_bottom + self.node_spacing;
                        if curr_pos.1 < min_y {
                            positions.get_mut(&curr).unwrap().1 = min_y;
                        }
                    }
                    LayoutDirection::Vertical => {
                        // Stacking direction is horizontal (x)
                        let prev_right = prev_pos.0 + prev_size.x;
                        let min_x = prev_right + self.node_spacing;
                        if curr_pos.0 < min_x {
                            positions.get_mut(&curr).unwrap().0 = min_x;
                        }
                    }
                }
            }
        }
    }
}

impl Layouter for HierarchicalLayouter {
    fn execute(&self, model: &GraphModel) -> HashMap<NodeIndex, (f32, f32)> {
        if model.node_count() == 0 {
            return HashMap::new();
        }

        // 1. Rank assignment
        let ranks = self.assign_ranks(model);

        // 2. Build layers from ranks
        let mut layers = self.build_layers(&ranks);

        // 3. Crossing minimization
        self.minimize_crossings(&mut layers, model);

        // 4. Coordinate assignment
        let mut positions = self.assign_coordinates(&layers, model);

        // 5. Enforce minimum spacing based on actual node sizes
        self.enforce_minimum_spacing(&mut positions, &layers, model);

        positions
    }
}

pub struct EdgeBundler;

impl EdgeBundler {
    pub fn bundle_edges(model: &GraphModel) -> Vec<Vec<codestory_core::EdgeId>> {
        let mut bundles: HashMap<(NodeIndex, NodeIndex), Vec<codestory_core::EdgeId>> =
            HashMap::new();

        for edge_idx in model.graph.edge_indices() {
            if let Some((source, target)) = model.graph.edge_endpoints(edge_idx) {
                let edge_data = &model.graph[edge_idx];
                bundles
                    .entry((source, target))
                    .or_default()
                    .push(edge_data.id);
            }
        }

        bundles.into_values().collect()
    }
}

/// Layout algorithm for nested hierarchical graphs with parent-child relationships.
///
/// This layouter uses a combination of hierarchical ranking and force-directed positioning
/// to create readable visualizations of nested structures.
pub struct NestingLayouter {
    /// Padding between parent container border and its children
    pub inner_padding: f32,
    /// Spacing between sibling nodes
    pub child_spacing: f32,
    /// Layout flow direction
    pub direction: LayoutDirection,
}

impl NestingLayouter {
    /// Default padding and spacing values optimized for readability
    pub const DEFAULT_INNER_PADDING: f32 = 10.0;
    pub const DEFAULT_CHILD_SPACING: f32 = 5.0;

    /// Maximum nesting depth to prevent stack overflow
    const MAX_NESTING_DEPTH: u32 = 100;

    /// Maximum iterations for ranking convergence
    const MAX_RANKING_ITERATIONS: usize = 1000;
}

impl NestingLayouter {
    #[allow(clippy::too_many_arguments)]
    fn layout_recursive(
        &self,
        model: &GraphModel,
        node_id: codestory_core::NodeId,
        x: f32,
        y: f32,
        positions: &mut HashMap<NodeIndex, (f32, f32)>,
        sizes: &mut HashMap<NodeIndex, Vec2>,
        depth: u32,
    ) -> Vec2 {
        // Prevent stack overflow from deeply nested or circular graphs
        if depth > Self::MAX_NESTING_DEPTH {
            tracing::warn!(
                "Maximum nesting depth ({}) exceeded for node {:?}, using default size",
                Self::MAX_NESTING_DEPTH,
                node_id
            );
            let node_idx = *model.node_map.get(&node_id).unwrap();
            return model.graph[node_idx].size;
        }
        let node_idx = *model.node_map.get(&node_id).unwrap();
        let node = &model.graph[node_idx];

        positions.insert(node_idx, (x, y));

        if !node.expanded || node.children.is_empty() {
            sizes.insert(node_idx, node.size);
            return node.size;
        }

        let start_y = y + 30.0 + self.inner_padding; // Header height + padding

        match node.group_layout {
            GroupLayout::LIST => {
                let mut current_y = start_y;
                let mut max_width = node.size.x;

                for &child_id in &node.children {
                    let child_size = self.layout_recursive(
                        model,
                        child_id,
                        x + self.inner_padding,
                        current_y,
                        positions,
                        sizes,
                        depth + 1,
                    );
                    current_y += child_size.y + self.child_spacing;
                    max_width = max_width.max(child_size.x + 2.0 * self.inner_padding);
                }

                let final_size = Vec2::new(
                    max_width,
                    (current_y - y + self.inner_padding).max(node.size.y),
                );
                sizes.insert(node_idx, final_size);
                final_size
            }
            GroupLayout::GRID => {
                let child_count = node.children.len();
                let cols = (child_count as f32).sqrt().ceil() as usize;

                let mut current_x = x + self.inner_padding;
                let mut current_y = start_y;
                let mut row_max_height = 0.0;
                let mut content_width: f32 = 0.0;

                for (i, &child_id) in node.children.iter().enumerate() {
                    if i > 0 && i % cols == 0 {
                        current_x = x + self.inner_padding;
                        current_y += row_max_height + self.child_spacing;
                        row_max_height = 0.0;
                    }

                    let child_size = self.layout_recursive(
                        model,
                        child_id,
                        current_x,
                        current_y,
                        positions,
                        sizes,
                        depth + 1,
                    );

                    current_x += child_size.x + self.child_spacing;
                    row_max_height = row_max_height.max(child_size.y);

                    content_width = content_width.max(current_x - x);
                }

                // Account for last row height
                current_y += row_max_height;

                let final_size = Vec2::new(
                    content_width.max(node.size.x) + self.inner_padding,
                    (current_y - y + self.inner_padding).max(node.size.y),
                );
                sizes.insert(node_idx, final_size);
                final_size
            }
        }
    }
}

impl Layouter for NestingLayouter {
    fn execute(&self, model: &GraphModel) -> HashMap<NodeIndex, (f32, f32)> {
        self.execute_enhanced(model).0
    }
}

impl EnhancedLayouter for NestingLayouter {
    fn execute_enhanced(
        &self,
        model: &GraphModel,
    ) -> (HashMap<NodeIndex, (f32, f32)>, HashMap<NodeIndex, Vec2>) {
        let mut positions = HashMap::new();
        let mut sizes = HashMap::new();

        // Get root nodes (nodes without parents)
        let root_nodes: Vec<_> = model
            .graph
            .node_indices()
            .filter(|&idx| model.graph[idx].parent.is_none())
            .collect();

        if root_nodes.is_empty() {
            return (positions, sizes);
        }

        // Assign layers/ranks to root nodes based on edges between them
        let mut ranks: HashMap<NodeIndex, i32> = HashMap::new();
        for &node in &root_nodes {
            ranks.insert(node, 0);
        }

        // Iteratively assign ranks based on edge dependencies
        let max_iterations = (root_nodes.len() + 2).min(Self::MAX_RANKING_ITERATIONS);
        let mut converged = false;
        for _iteration in 0..max_iterations {
            let mut changed = false;
            for edge_idx in model.graph.edge_indices() {
                if let Some((source, target)) = model.graph.edge_endpoints(edge_idx) {
                    let source_root = Self::find_root(model, source);
                    let target_root = Self::find_root(model, target);

                    if source_root == target_root {
                        continue;
                    }

                    if let (Some(&source_rank), Some(&target_rank)) =
                        (ranks.get(&source_root), ranks.get(&target_root))
                        && target_rank <= source_rank
                    {
                        ranks.insert(target_root, source_rank + 1);
                        changed = true;
                    }
                }
            }
            if !changed {
                converged = true;
                break;
            }
        }

        if !converged {
            tracing::warn!(
                "Root node ranking did not converge after {} iterations",
                max_iterations
            );
        }

        // Group root nodes by layer
        let mut layers: HashMap<i32, Vec<NodeIndex>> = HashMap::new();
        for (&node, &rank) in &ranks {
            layers.entry(rank).or_default().push(node);
        }

        // --- Crossing Minimization (Barycenter Heuristic) ---
        // Initial order: sort by name
        for nodes in layers.values_mut() {
            nodes.sort_by_key(|&n| model.graph[n].name.clone());
        }

        let mut sorted_ranks: Vec<_> = layers.keys().cloned().collect();
        sorted_ranks.sort();

        // Assign initial layer coordinates for barycenter calculation
        // If Vertical, these are X. If Horizontal, these are Y.
        let mut layer_coords: HashMap<NodeIndex, f32> = HashMap::new();
        for rank in &sorted_ranks {
            if let Some(layer_nodes) = layers.get(rank) {
                for (j, &node_idx) in layer_nodes.iter().enumerate() {
                    layer_coords.insert(node_idx, j as f32 * 150.0);
                }
            }
        }

        // Barycenter passes (2 down, 2 up)
        for _ in 0..2 {
            // Down pass
            for &rank in sorted_ranks.iter().skip(1) {
                if let Some(layer_nodes) = layers.get_mut(&rank) {
                    Self::order_by_barycenter(layer_nodes, model, &layer_coords, true);
                    // Update layer_coords after reordering
                    for (j, &node_idx) in layer_nodes.iter().enumerate() {
                        layer_coords.insert(node_idx, j as f32 * 150.0);
                    }
                }
            }
            // Up pass
            for i in (0..sorted_ranks.len().saturating_sub(1)).rev() {
                let rank = sorted_ranks[i];
                if let Some(layer_nodes) = layers.get_mut(&rank) {
                    Self::order_by_barycenter(layer_nodes, model, &layer_coords, false);
                    for (j, &node_idx) in layer_nodes.iter().enumerate() {
                        layer_coords.insert(node_idx, j as f32 * 150.0);
                    }
                }
            }
        }

        // --- Initial Position Assignment ---
        let layer_spacing = 300.0;
        let node_spacing = 150.0;

        for rank in &sorted_ranks {
            if let Some(layer_nodes) = layers.get(rank) {
                // Calculate sizes first
                let mut layer_sizes: Vec<Vec2> = Vec::new();
                for &node_idx in layer_nodes {
                    let node_id = model.graph[node_idx].id;
                    let size = self.layout_recursive(
                        model,
                        node_id,
                        0.0,
                        0.0,
                        &mut HashMap::new(),
                        &mut HashMap::new(),
                        0,
                    );
                    layer_sizes.push(size);
                }

                // Calculate total extent for centering the layer
                let extent: f32 = match self.direction {
                    LayoutDirection::Vertical => {
                        layer_sizes.iter().map(|s| s.x).sum::<f32>()
                            + (layer_nodes.len().saturating_sub(1) as f32) * node_spacing
                    }
                    LayoutDirection::Horizontal => {
                        layer_sizes.iter().map(|s| s.y).sum::<f32>()
                            + (layer_nodes.len().saturating_sub(1) as f32) * node_spacing
                    }
                };

                let mut current_offset = -extent / 2.0;
                let rank_pos = *rank as f32 * layer_spacing;

                for &node_idx in layer_nodes.iter() {
                    let node_id = model.graph[node_idx].id;
                    match self.direction {
                        LayoutDirection::Vertical => {
                            let size = self.layout_recursive(
                                model,
                                node_id,
                                current_offset,
                                rank_pos,
                                &mut positions,
                                &mut sizes,
                                0,
                            );
                            current_offset += size.x + node_spacing;
                        }
                        LayoutDirection::Horizontal => {
                            let size = self.layout_recursive(
                                model,
                                node_id,
                                rank_pos,
                                current_offset,
                                &mut positions,
                                &mut sizes,
                                0,
                            );
                            current_offset += size.y + node_spacing;
                        }
                    }
                }
            }
        }

        // --- Force-Directed Post-Pass ---
        tracing::debug!(
            "Layout: {} layers, {} root nodes, {} total positions. Applying force-directed pass",
            sorted_ranks.len(),
            root_nodes.len(),
            positions.len()
        );
        self.apply_force_directed(&mut positions, &sizes, model, 5);

        (positions, sizes)
    }
}

impl NestingLayouter {
    fn find_root(model: &GraphModel, node_idx: NodeIndex) -> NodeIndex {
        let node = &model.graph[node_idx];
        match node.parent {
            Some(parent_id) => {
                if let Some(&parent_idx) = model.node_map.get(&parent_id) {
                    Self::find_root(model, parent_idx)
                } else {
                    node_idx
                }
            }
            None => node_idx,
        }
    }

    /// Order nodes in a layer by barycenter of their neighbors in adjacent layer.
    fn order_by_barycenter(
        layer_nodes: &mut [NodeIndex],
        model: &GraphModel,
        layer_coords: &HashMap<NodeIndex, f32>,
        use_predecessors: bool,
    ) {
        let mut barycenters: HashMap<NodeIndex, f32> = HashMap::new();

        for &node_idx in layer_nodes.iter() {
            let node_root = Self::find_root(model, node_idx);
            let mut sum_x = 0.0;
            let mut count = 0;

            for edge_idx in model.graph.edge_indices() {
                if let Some((source, target)) = model.graph.edge_endpoints(edge_idx) {
                    let source_root = Self::find_root(model, source);
                    let target_root = Self::find_root(model, target);

                    let neighbor_root = if use_predecessors {
                        if target_root == node_root {
                            Some(source_root)
                        } else {
                            None
                        }
                    } else if source_root == node_root {
                        Some(target_root)
                    } else {
                        None
                    };

                    if let Some(nr) = neighbor_root
                        && let Some(&coord) = layer_coords.get(&nr)
                    {
                        sum_x += coord;
                        count += 1;
                    }
                }
            }

            let barycenter = if count > 0 {
                sum_x / count as f32
            } else {
                *layer_coords.get(&node_root).unwrap_or(&0.0)
            };
            barycenters.insert(node_idx, barycenter);
        }

        layer_nodes.sort_by(|a, b| {
            barycenters
                .get(a)
                .unwrap_or(&0.0)
                .partial_cmp(barycenters.get(b).unwrap_or(&0.0))
                .unwrap()
        });
    }

    /// Apply simple force-directed repulsion to spread out overlapping nodes.
    fn apply_force_directed(
        &self,
        positions: &mut HashMap<NodeIndex, (f32, f32)>,
        sizes: &HashMap<NodeIndex, Vec2>,
        model: &GraphModel,
        iterations: usize,
    ) {
        let repulsion_strength = 500.0;
        let min_distance = 20.0;

        let mut node_indices: Vec<NodeIndex> = positions.keys().cloned().collect();
        // Sort indices to ensure deterministic iteration order for force calculation
        node_indices.sort();

        for _ in 0..iterations {
            let mut forces: HashMap<NodeIndex, (f32, f32)> = HashMap::new();

            for &node_idx in &node_indices {
                forces.insert(node_idx, (0.0, 0.0));
            }

            // Calculate repulsion forces between all node pairs
            for i in 0..node_indices.len() {
                for j in (i + 1)..node_indices.len() {
                    let a = node_indices[i];
                    let b = node_indices[j];

                    // Skip if same hierarchy (parent-child)
                    let a_parent = model.graph[a].parent;
                    let b_parent = model.graph[b].parent;
                    if a_parent == Some(model.graph[b].id) || b_parent == Some(model.graph[a].id) {
                        continue;
                    }

                    let (ax, ay) = positions[&a];
                    let (bx, by) = positions[&b];
                    let a_size = sizes.get(&a).cloned().unwrap_or(Vec2::new(100.0, 30.0));
                    let b_size = sizes.get(&b).cloned().unwrap_or(Vec2::new(100.0, 30.0));

                    // Use center positions
                    let acx = ax + a_size.x / 2.0;
                    let acy = ay + a_size.y / 2.0;
                    let bcx = bx + b_size.x / 2.0;
                    let bcy = by + b_size.y / 2.0;

                    let dx = acx - bcx;
                    let dy = acy - bcy;
                    let dist = (dx * dx + dy * dy).sqrt().max(min_distance);

                    // Only apply horizontal repulsion to avoid breaking layer structure
                    let force = repulsion_strength / (dist * dist);
                    let fx = if dx.abs() > 0.01 {
                        force * dx.signum()
                    } else {
                        0.0
                    };

                    if let Some(f) = forces.get_mut(&a) {
                        f.0 += fx;
                    }
                    if let Some(f) = forces.get_mut(&b) {
                        f.0 -= fx;
                    }
                }
            }

            // Apply forces with damping
            let damping = 0.3;
            for &node_idx in &node_indices {
                if let Some(&(fx, fy)) = forces.get(&node_idx)
                    && let Some(pos) = positions.get_mut(&node_idx)
                {
                    match self.direction {
                        LayoutDirection::Vertical => {
                            pos.0 += fx * damping;
                        }
                        LayoutDirection::Horizontal => {
                            pos.1 += fy * damping;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphModel;
    use codestory_core::{Edge, EdgeKind, Node, NodeId, NodeKind};

    // ---- Helper to build a simple DAG model ----
    fn build_chain_model(n: usize) -> GraphModel {
        let mut model = GraphModel::new();
        for i in 0..n {
            model.add_node(Node {
                id: NodeId(i as i64),
                kind: NodeKind::FUNCTION,
                serialized_name: format!("N{}", i),
                ..Default::default()
            });
        }
        for i in 0..n.saturating_sub(1) {
            model.add_edge(Edge {
                id: codestory_core::EdgeId(i as i64),
                source: NodeId(i as i64),
                target: NodeId((i + 1) as i64),
                kind: EdgeKind::CALL,
                ..Default::default()
            });
        }
        model
    }

    fn build_diamond_model() -> GraphModel {
        // A -> B, A -> C, B -> D, C -> D
        let mut model = GraphModel::new();
        for (i, name) in [(0, "A"), (1, "B"), (2, "C"), (3, "D")].iter() {
            model.add_node(Node {
                id: NodeId(*i),
                kind: NodeKind::FUNCTION,
                serialized_name: name.to_string(),
                ..Default::default()
            });
        }
        let edges = [(0, 1), (0, 2), (1, 3), (2, 3)];
        for (idx, (s, t)) in edges.iter().enumerate() {
            model.add_edge(Edge {
                id: codestory_core::EdgeId(idx as i64),
                source: NodeId(*s),
                target: NodeId(*t),
                kind: EdgeKind::CALL,
                ..Default::default()
            });
        }
        model
    }

    // ---- HierarchicalLayouter unit tests ----

    #[test]
    fn test_hierarchical_rank_assignment_chain() {
        let model = build_chain_model(4);
        let layouter = HierarchicalLayouter::default();
        let ranks = layouter.assign_ranks(&model);

        for i in 0..3 {
            let src = *model.node_map.get(&NodeId(i as i64)).unwrap();
            let tgt = *model.node_map.get(&NodeId((i + 1) as i64)).unwrap();
            assert!(
                ranks[&tgt] > ranks[&src],
                "rank(N{}) = {} should be > rank(N{}) = {}",
                i + 1,
                ranks[&tgt],
                i,
                ranks[&src]
            );
        }
    }

    #[test]
    fn test_hierarchical_root_placement() {
        let model = build_diamond_model();
        let layouter = HierarchicalLayouter::default();
        let ranks = layouter.assign_ranks(&model);

        // A (NodeId(0)) has no incoming edges -> should be rank 0
        let a_idx = *model.node_map.get(&NodeId(0)).unwrap();
        assert_eq!(ranks[&a_idx], 0, "Root node A should have rank 0");
    }

    #[test]
    fn test_hierarchical_diamond_ranks() {
        let model = build_diamond_model();
        let layouter = HierarchicalLayouter::default();
        let ranks = layouter.assign_ranks(&model);

        let rank_of = |id: i64| {
            let idx = *model.node_map.get(&NodeId(id)).unwrap();
            ranks[&idx]
        };

        assert_eq!(rank_of(0), 0); // A
        assert_eq!(rank_of(1), 1); // B
        assert_eq!(rank_of(2), 1); // C
        assert_eq!(rank_of(3), 2); // D
    }

    #[test]
    fn test_hierarchical_full_layout_horizontal() {
        let model = build_chain_model(3);
        let layouter = HierarchicalLayouter {
            direction: LayoutDirection::Horizontal,
            layer_spacing: 150.0,
            node_spacing: 50.0,
            rank_algorithm: RankAlgorithm::LongestPath,
        };
        let positions = layouter.execute(&model);

        assert_eq!(positions.len(), 3);

        // In horizontal mode, x increases with rank
        let pos = |id: i64| {
            let idx = *model.node_map.get(&NodeId(id)).unwrap();
            positions[&idx]
        };

        assert!(pos(1).0 > pos(0).0, "N1 should be right of N0");
        assert!(pos(2).0 > pos(1).0, "N2 should be right of N1");
    }

    #[test]
    fn test_hierarchical_full_layout_vertical() {
        let model = build_chain_model(3);
        let layouter = HierarchicalLayouter {
            direction: LayoutDirection::Vertical,
            layer_spacing: 150.0,
            node_spacing: 50.0,
            rank_algorithm: RankAlgorithm::LongestPath,
        };
        let positions = layouter.execute(&model);

        let pos = |id: i64| {
            let idx = *model.node_map.get(&NodeId(id)).unwrap();
            positions[&idx]
        };

        assert!(pos(1).1 > pos(0).1, "N1 should be below N0");
        assert!(pos(2).1 > pos(1).1, "N2 should be below N1");
    }

    #[test]
    fn test_hierarchical_direction_transposition() {
        let model = build_diamond_model();
        let h_layouter = HierarchicalLayouter {
            direction: LayoutDirection::Horizontal,
            layer_spacing: 150.0,
            node_spacing: 50.0,
            rank_algorithm: RankAlgorithm::LongestPath,
        };
        let v_layouter = HierarchicalLayouter {
            direction: LayoutDirection::Vertical,
            layer_spacing: 150.0,
            node_spacing: 50.0,
            rank_algorithm: RankAlgorithm::LongestPath,
        };

        // Test transposition on raw coordinate assignment (before size-aware spacing)
        let ranks = h_layouter.assign_ranks(&model);
        let mut h_layers = h_layouter.build_layers(&ranks);
        h_layouter.minimize_crossings(&mut h_layers, &model);
        let h_positions = h_layouter.assign_coordinates(&h_layers, &model);

        let v_ranks = v_layouter.assign_ranks(&model);
        let mut v_layers = v_layouter.build_layers(&v_ranks);
        v_layouter.minimize_crossings(&mut v_layers, &model);
        let v_positions = v_layouter.assign_coordinates(&v_layers, &model);

        for node_idx in model.graph.node_indices() {
            let (hx, hy) = h_positions[&node_idx];
            let (vx, vy) = v_positions[&node_idx];
            assert!(
                (hx - vy).abs() < 0.01 && (hy - vx).abs() < 0.01,
                "Horizontal ({}, {}) should transpose to Vertical ({}, {})",
                hx,
                hy,
                vx,
                vy
            );
        }
    }

    #[test]
    fn test_hierarchical_empty_graph() {
        let model = GraphModel::new();
        let layouter = HierarchicalLayouter::default();
        let positions = layouter.execute(&model);
        assert!(positions.is_empty());
    }

    #[test]
    fn test_hierarchical_single_node() {
        let mut model = GraphModel::new();
        model.add_node(Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "Solo".to_string(),
            ..Default::default()
        });
        let layouter = HierarchicalLayouter::default();
        let positions = layouter.execute(&model);
        assert_eq!(positions.len(), 1);

        let ranks = layouter.assign_ranks(&model);
        let idx = *model.node_map.get(&NodeId(1)).unwrap();
        assert_eq!(ranks[&idx], 0);
    }

    #[test]
    fn test_hierarchical_crossing_minimization() {
        // Test that barycenter method runs without error on diamond graph
        let model = build_diamond_model();
        let layouter = HierarchicalLayouter::default();
        let ranks = layouter.assign_ranks(&model);
        let mut layers = layouter.build_layers(&ranks);
        layouter.minimize_crossings(&mut layers, &model);

        // All nodes should still be present
        let total: usize = layers.iter().map(|l| l.len()).sum();
        assert_eq!(total, 4);
    }

    #[test]
    fn test_hierarchical_minimum_spacing() {
        let model = build_diamond_model();
        let layouter = HierarchicalLayouter {
            direction: LayoutDirection::Horizontal,
            layer_spacing: 150.0,
            node_spacing: 50.0,
            rank_algorithm: RankAlgorithm::LongestPath,
        };
        let positions = layouter.execute(&model);
        let ranks = layouter.assign_ranks(&model);

        // Check layer spacing (horizontal distance between layers)
        let node_indices: Vec<NodeIndex> = model.graph.node_indices().collect();
        for i in 0..node_indices.len() {
            for j in (i + 1)..node_indices.len() {
                let ni = node_indices[i];
                let nj = node_indices[j];
                let ri = ranks[&ni];
                let rj = ranks[&nj];

                if ri != rj {
                    // Different layers: horizontal distance >= layer_spacing
                    let dx = (positions[&ni].0 - positions[&nj].0).abs();
                    assert!(
                        dx >= layouter.layer_spacing - 0.01,
                        "Layer spacing violated: dx={} between rank {} and rank {}",
                        dx,
                        ri,
                        rj
                    );
                }
            }
        }
    }

    #[test]
    fn test_grid_layout() {
        let mut model = GraphModel::new();
        for i in 0..4 {
            model.add_node(Node {
                id: NodeId(i),
                kind: NodeKind::UNKNOWN,
                serialized_name: format!("N{}", i),
                ..Default::default()
            });
        }

        let layouter = GridLayouter { spacing: 100.0 };
        let positions = layouter.execute(&model);

        assert_eq!(positions.len(), 4);
        let mut coords: Vec<_> = positions.values().cloned().collect();
        coords.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap()
                .then(a.1.partial_cmp(&b.1).unwrap())
        });

        assert_eq!(coords[0], (0.0, 0.0));
        assert_eq!(coords[3], (100.0, 100.0));
    }

    #[test]
    fn test_nesting_layouter_direction() {
        let mut model = GraphModel::new();
        model.add_node(Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "A".to_string(),
            ..Default::default()
        });
        model.add_node(Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "B".to_string(),
            ..Default::default()
        });
        model.add_edge(Edge {
            id: codestory_core::EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::CALL,
            ..Default::default()
        });
        model.rebuild_hierarchy();

        // Test Vertical (default)
        let layouter_v = NestingLayouter {
            inner_padding: 0.0,
            child_spacing: 0.0,
            direction: LayoutDirection::Vertical,
        };
        let (pos_v, _) = layouter_v.execute_enhanced(&model);

        let node1_idx = *model.node_map.get(&NodeId(1)).unwrap();
        let node2_idx = *model.node_map.get(&NodeId(2)).unwrap();

        let y1 = pos_v[&node1_idx].1;
        let y2 = pos_v[&node2_idx].1;
        assert!(
            (y2 - y1).abs() >= 300.0,
            "Vertical distance should be at least layer_spacing"
        );

        // Test Horizontal
        let layouter_h = NestingLayouter {
            inner_padding: 0.0,
            child_spacing: 0.0,
            direction: LayoutDirection::Horizontal,
        };
        let (pos_h, _) = layouter_h.execute_enhanced(&model);

        let x1 = pos_h[&node1_idx].0;
        let x2 = pos_h[&node2_idx].0;
        assert!(
            (x2 - x1).abs() >= 300.0,
            "Horizontal distance should be at least layer_spacing"
        );
    }

    #[test]
    fn test_trail_layouter_direction() {
        let mut node_sizes = HashMap::new();
        node_sizes.insert(NodeId(1), Vec2::new(100.0, 30.0));
        node_sizes.insert(NodeId(2), Vec2::new(100.0, 30.0));

        let mut model = GraphModel::new();
        model.add_node(Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "A".to_string(),
            ..Default::default()
        });
        model.add_node(Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "B".to_string(),
            ..Default::default()
        });
        model.add_edge(Edge {
            id: codestory_core::EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::CALL,
            ..Default::default()
        });

        // Test Horizontal (default)
        let layouter_h = TrailLayouter {
            node_sizes: node_sizes.clone(),
            layer_spacing: 200.0,
            node_spacing: 50.0,
            direction: LayoutDirection::Horizontal,
        };
        let pos_h = layouter_h.execute(&model);
        let node1_idx = *model.node_map.get(&NodeId(1)).unwrap();
        let node2_idx = *model.node_map.get(&NodeId(2)).unwrap();

        let x1 = pos_h[&node1_idx].0;
        let x2 = pos_h[&node2_idx].0;
        assert!(
            (x2 - x1).abs() >= 200.0,
            "Horizontal distance in TrailLayouter should be at least layer_spacing"
        );

        // Test Vertical
        let layouter_v = TrailLayouter {
            node_sizes,
            layer_spacing: 200.0,
            node_spacing: 50.0,
            direction: LayoutDirection::Vertical,
        };
        let pos_v = layouter_v.execute(&model);
        let y1 = pos_v[&node1_idx].1;
        let y2 = pos_v[&node2_idx].1;
        assert!(
            (y2 - y1).abs() >= 200.0,
            "Vertical distance in TrailLayouter should be at least layer_spacing"
        );
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use crate::graph::GraphModel;
    use codestory_core::{Edge, EdgeKind, Node, NodeId, NodeKind};
    use proptest::prelude::*;

    /// Build a random DAG with `n` nodes and a subset of forward edges.
    /// Edges only go from lower-id to higher-id to ensure acyclicity.
    fn build_random_dag(n: usize, edges: Vec<(usize, usize)>) -> GraphModel {
        let mut model = GraphModel::new();
        for i in 0..n {
            model.add_node(Node {
                id: NodeId(i as i64),
                kind: NodeKind::FUNCTION,
                serialized_name: format!("N{}", i),
                ..Default::default()
            });
        }
        for (idx, (s, t)) in edges.iter().enumerate() {
            // Only add forward edges (s < t) to maintain DAG property
            let (src, tgt) = if s <= t { (*s, *t) } else { (*t, *s) };
            if src != tgt && src < n && tgt < n {
                model.add_edge(Edge {
                    id: codestory_core::EdgeId(idx as i64),
                    source: NodeId(src as i64),
                    target: NodeId(tgt as i64),
                    kind: EdgeKind::CALL,
                    ..Default::default()
                });
            }
        }
        model
    }

    /// Strategy for generating a DAG with 2..20 nodes and some edges
    fn dag_strategy() -> impl Strategy<Value = GraphModel> {
        (2..20usize).prop_flat_map(|n| {
            let max_edges = n * (n - 1) / 2; // max forward edges
            let edge_count = 1..=max_edges.max(1);
            (Just(n), proptest::collection::vec((0..n, 0..n), edge_count)).prop_map(
                |(n, edges)| build_random_dag(n, edges),
            )
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 100,
            max_shrink_iters: 1000,
            ..ProptestConfig::default()
        })]

        /// Property 14: For any directed edge (A -> B), rank(B) > rank(A)
        #[test]
        fn prop_rank_assignment_respects_edges(model in dag_strategy()) {
            let layouter = HierarchicalLayouter::default();
            let ranks = layouter.assign_ranks(&model);

            for edge_idx in model.graph.edge_indices() {
                if let Some((source, target)) = model.graph.edge_endpoints(edge_idx) {
                    if source == target {
                        continue;
                    }
                    let sr = ranks[&source];
                    let tr = ranks[&target];
                    prop_assert!(
                        tr > sr,
                        "Edge from {:?} (rank {}) to {:?} (rank {}) violates rank ordering",
                        source, sr, target, tr
                    );
                }
            }
        }

        /// Property 15: Minimum spacing between nodes
        /// - Same-layer nodes: vertical distance >= node_spacing (50px) between bounding boxes
        /// - Adjacent layers: horizontal distance >= layer_spacing (150px)
        #[test]
        fn prop_minimum_spacing(model in dag_strategy()) {
            let layer_spacing = 150.0f32;
            let node_spacing = 50.0f32;
            let layouter = HierarchicalLayouter {
                direction: LayoutDirection::Horizontal,
                layer_spacing,
                node_spacing,
                rank_algorithm: RankAlgorithm::LongestPath,
            };
            let positions = layouter.execute(&model);
            let ranks = layouter.assign_ranks(&model);

            let node_indices: Vec<NodeIndex> = model.graph.node_indices().collect();
            for i in 0..node_indices.len() {
                for j in (i + 1)..node_indices.len() {
                    let ni = node_indices[i];
                    let nj = node_indices[j];
                    let ri = ranks[&ni];
                    let rj = ranks[&nj];

                    if ri != rj {
                        // Different layers: horizontal distance >= layer_spacing
                        let dx = (positions[&ni].0 - positions[&nj].0).abs();
                        let expected = ((ri - rj).unsigned_abs() as f32) * layer_spacing;
                        prop_assert!(
                            dx >= expected - 0.01,
                            "Layer spacing violated: dx={} < expected={} between ranks {} and {}",
                            dx, expected, ri, rj
                        );
                    }
                    // Same layer: vertical distance between node bounding box edges >= node_spacing
                    // We check that either:
                    //   pos_j.y >= pos_i.y + size_i.y + node_spacing, or
                    //   pos_i.y >= pos_j.y + size_j.y + node_spacing
                    // (i.e. they don't overlap with less than node_spacing gap)
                    if ri == rj {
                        let (_, yi) = positions[&ni];
                        let (_, yj) = positions[&nj];
                        let si = model.graph[ni].size.y;
                        let sj = model.graph[nj].size.y;

                        // Bounding boxes: [yi, yi+si] and [yj, yj+sj]
                        let gap = if yi <= yj {
                            yj - (yi + si)
                        } else {
                            yi - (yj + sj)
                        };
                        prop_assert!(
                            gap >= node_spacing - 0.01,
                            "Same-layer spacing violated: gap={} < {} between nodes at y={} (h={}) and y={} (h={})",
                            gap, node_spacing, yi, si, yj, sj
                        );
                    }
                }
            }
        }

        /// Property 16: Nodes with zero incoming edges get rank 0 (leftmost)
        #[test]
        fn prop_root_nodes_rank_zero(model in dag_strategy()) {
            let layouter = HierarchicalLayouter::default();
            let ranks = layouter.assign_ranks(&model);

            // Find nodes with zero incoming edges
            let mut has_incoming: std::collections::HashSet<NodeIndex> = std::collections::HashSet::new();
            for edge_idx in model.graph.edge_indices() {
                if let Some((source, target)) = model.graph.edge_endpoints(edge_idx) {
                    if source != target {
                        has_incoming.insert(target);
                    }
                }
            }

            for node_idx in model.graph.node_indices() {
                if !has_incoming.contains(&node_idx) {
                    prop_assert_eq!(
                        ranks[&node_idx], 0,
                        "Node {:?} has no incoming edges but rank = {} (expected 0)",
                        node_idx, ranks[&node_idx]
                    );
                }
            }
        }

        /// Property 17: Horizontal layout (x,y) corresponds to vertical layout (y,x) transposition
        /// Tests raw coordinate assignment before size-aware spacing enforcement.
        #[test]
        fn prop_direction_transposition(model in dag_strategy()) {
            let h_layouter = HierarchicalLayouter {
                direction: LayoutDirection::Horizontal,
                layer_spacing: 150.0,
                node_spacing: 50.0,
                rank_algorithm: RankAlgorithm::LongestPath,
            };
            let v_layouter = HierarchicalLayouter {
                direction: LayoutDirection::Vertical,
                layer_spacing: 150.0,
                node_spacing: 50.0,
                rank_algorithm: RankAlgorithm::LongestPath,
            };

            // Use raw coordinate assignment (before size-aware spacing enforcement)
            let h_ranks = h_layouter.assign_ranks(&model);
            let mut h_layers = h_layouter.build_layers(&h_ranks);
            h_layouter.minimize_crossings(&mut h_layers, &model);
            let h_positions = h_layouter.assign_coordinates(&h_layers, &model);

            let v_ranks = v_layouter.assign_ranks(&model);
            let mut v_layers = v_layouter.build_layers(&v_ranks);
            v_layouter.minimize_crossings(&mut v_layers, &model);
            let v_positions = v_layouter.assign_coordinates(&v_layers, &model);

            for node_idx in model.graph.node_indices() {
                let (hx, hy) = h_positions[&node_idx];
                let (vx, vy) = v_positions[&node_idx];
                prop_assert!(
                    (hx - vy).abs() < 0.01 && (hy - vx).abs() < 0.01,
                    "Transposition violated for {:?}: H=({}, {}), V=({}, {})",
                    node_idx, hx, hy, vx, vy
                );
            }
        }
    }
}
