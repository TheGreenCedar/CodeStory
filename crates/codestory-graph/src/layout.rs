use crate::graph::{GraphModel, GroupLayout, NodeIndex, Vec2};
use codestory_core::{LayoutDirection, NodeId};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

pub trait Layouter {
    fn execute(
        &self,
        model: &GraphModel,
    ) -> (HashMap<NodeIndex, (f32, f32)>, HashMap<NodeIndex, Vec2>);
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
/// This layouter uses hierarchical ranking for root nodes plus a local force pass
/// to improve readability in dense neighborhoods.
pub struct NestingLayouter {
    /// Padding between parent container border and its children
    pub inner_padding: f32,
    /// Spacing between sibling nodes
    pub child_spacing: f32,
    /// Layout flow direction
    pub direction: LayoutDirection,
}

#[derive(Default)]
struct RootRelations {
    root_edges: Vec<(NodeIndex, NodeIndex)>,
    incoming: HashMap<NodeIndex, Vec<NodeIndex>>,
    outgoing: HashMap<NodeIndex, Vec<NodeIndex>>,
}

impl NestingLayouter {
    /// Default padding and spacing values optimized for readability
    pub const DEFAULT_INNER_PADDING: f32 = 10.0;
    pub const DEFAULT_CHILD_SPACING: f32 = 5.0;
    const DEFAULT_NODE_WIDTH: f32 = 100.0;
    const DEFAULT_NODE_HEIGHT: f32 = 30.0;

    /// Maximum nesting depth to prevent stack overflow
    const MAX_NESTING_DEPTH: u32 = 100;
    /// Maximum iterations for ranking convergence
    const MAX_RANKING_ITERATIONS: usize = 1000;

    fn default_node_size() -> Vec2 {
        Vec2::new(Self::DEFAULT_NODE_WIDTH, Self::DEFAULT_NODE_HEIGHT)
    }

    fn root_nodes(model: &GraphModel) -> Vec<NodeIndex> {
        model
            .graph
            .node_indices()
            .filter(|&idx| model.graph[idx].parent.is_none())
            .collect()
    }

    fn resolve_root_cached(
        model: &GraphModel,
        node_idx: NodeIndex,
        cache: &mut HashMap<NodeIndex, NodeIndex>,
    ) -> NodeIndex {
        if let Some(&cached) = cache.get(&node_idx) {
            return cached;
        }

        let mut trail = Vec::new();
        let mut seen = HashSet::new();
        let mut current = node_idx;

        loop {
            if let Some(&cached) = cache.get(&current) {
                for idx in trail {
                    cache.insert(idx, cached);
                }
                return cached;
            }

            if !seen.insert(current) {
                // Fallback for malformed cyclic parent chains.
                for idx in trail {
                    cache.insert(idx, current);
                }
                return current;
            }

            trail.push(current);
            let parent_idx = model.graph[current]
                .parent
                .and_then(|parent_id| model.node_map.get(&parent_id).copied());

            match parent_idx {
                Some(parent_idx) => {
                    current = parent_idx;
                }
                None => {
                    for idx in trail {
                        cache.insert(idx, current);
                    }
                    return current;
                }
            }
        }
    }

    fn build_node_roots(model: &GraphModel) -> HashMap<NodeIndex, NodeIndex> {
        let mut node_roots = HashMap::with_capacity(model.graph.node_count());
        for node_idx in model.graph.node_indices() {
            let root = Self::resolve_root_cached(model, node_idx, &mut node_roots);
            node_roots.insert(node_idx, root);
        }
        node_roots
    }

    fn build_root_relations(
        model: &GraphModel,
        node_roots: &HashMap<NodeIndex, NodeIndex>,
    ) -> RootRelations {
        let mut relations = RootRelations::default();

        for edge_idx in model.graph.edge_indices() {
            if let Some((source, target)) = model.graph.edge_endpoints(edge_idx) {
                let Some(&source_root) = node_roots.get(&source) else {
                    continue;
                };
                let Some(&target_root) = node_roots.get(&target) else {
                    continue;
                };

                if source_root == target_root {
                    continue;
                }

                relations.root_edges.push((source_root, target_root));
                relations
                    .incoming
                    .entry(target_root)
                    .or_default()
                    .push(source_root);
                relations
                    .outgoing
                    .entry(source_root)
                    .or_default()
                    .push(target_root);
            }
        }

        relations
    }

    fn assign_root_ranks(
        root_nodes: &[NodeIndex],
        relations: &RootRelations,
    ) -> HashMap<NodeIndex, i32> {
        let mut ranks = HashMap::with_capacity(root_nodes.len());
        for &node in root_nodes {
            ranks.insert(node, 0);
        }

        let max_iterations = (root_nodes.len() + 2).min(Self::MAX_RANKING_ITERATIONS);
        let mut converged = false;
        for _ in 0..max_iterations {
            let mut changed = false;
            for &(source_root, target_root) in &relations.root_edges {
                if let (Some(&source_rank), Some(&target_rank)) =
                    (ranks.get(&source_root), ranks.get(&target_root))
                    && target_rank <= source_rank
                {
                    ranks.insert(target_root, source_rank + 1);
                    changed = true;
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

        Self::compress_ranks(&mut ranks);
        ranks
    }

    fn compress_ranks(ranks: &mut HashMap<NodeIndex, i32>) {
        if ranks.is_empty() {
            return;
        }

        let mut unique_ranks: Vec<i32> = ranks.values().copied().collect();
        unique_ranks.sort_unstable();
        unique_ranks.dedup();

        let mut remap: HashMap<i32, i32> = HashMap::new();
        for (i, rank) in unique_ranks.iter().enumerate() {
            remap.insert(*rank, i as i32);
        }

        for rank in ranks.values_mut() {
            if let Some(new_rank) = remap.get(rank) {
                *rank = *new_rank;
            }
        }
    }

    fn build_layers(
        model: &GraphModel,
        ranks: &HashMap<NodeIndex, i32>,
    ) -> HashMap<i32, Vec<NodeIndex>> {
        let mut layers: HashMap<i32, Vec<NodeIndex>> = HashMap::new();
        for (&node, &rank) in ranks {
            layers.entry(rank).or_default().push(node);
        }

        for nodes in layers.values_mut() {
            nodes.sort_by(|a, b| model.graph[*a].name.cmp(&model.graph[*b].name));
        }

        layers
    }

    fn sorted_ranks(layers: &HashMap<i32, Vec<NodeIndex>>) -> Vec<i32> {
        let mut sorted_ranks: Vec<_> = layers.keys().copied().collect();
        sorted_ranks.sort_unstable();
        sorted_ranks
    }

    fn spacing_for_root_count(root_count: usize) -> (f32, f32, f32) {
        let tiny_graph = root_count <= 4;
        let small_graph = root_count <= 12;

        let barycenter_spacing = if tiny_graph {
            80.0
        } else if small_graph {
            100.0
        } else {
            150.0
        };
        let layer_spacing = if tiny_graph {
            120.0
        } else if small_graph {
            160.0
        } else {
            300.0
        };
        let node_spacing = if tiny_graph {
            60.0
        } else if small_graph {
            80.0
        } else {
            150.0
        };

        (barycenter_spacing, layer_spacing, node_spacing)
    }

    fn initialize_layer_coords(
        layers: &HashMap<i32, Vec<NodeIndex>>,
        sorted_ranks: &[i32],
        barycenter_spacing: f32,
    ) -> HashMap<NodeIndex, f32> {
        let mut layer_coords: HashMap<NodeIndex, f32> = HashMap::new();
        for rank in sorted_ranks {
            if let Some(layer_nodes) = layers.get(rank) {
                for (j, &node_idx) in layer_nodes.iter().enumerate() {
                    layer_coords.insert(node_idx, j as f32 * barycenter_spacing);
                }
            }
        }
        layer_coords
    }

    fn order_layer_by_barycenter(
        layer_nodes: &mut [NodeIndex],
        layer_coords: &HashMap<NodeIndex, f32>,
        neighbors_by_root: &HashMap<NodeIndex, Vec<NodeIndex>>,
    ) {
        let mut barycenters: HashMap<NodeIndex, f32> = HashMap::new();

        for &node_idx in layer_nodes.iter() {
            let mut sum = 0.0;
            let mut count = 0;

            if let Some(neighbors) = neighbors_by_root.get(&node_idx) {
                for &neighbor in neighbors {
                    if let Some(&coord) = layer_coords.get(&neighbor) {
                        sum += coord;
                        count += 1;
                    }
                }
            }

            let barycenter = if count > 0 {
                sum / count as f32
            } else {
                *layer_coords.get(&node_idx).unwrap_or(&0.0)
            };
            barycenters.insert(node_idx, barycenter);
        }

        layer_nodes.sort_by(|a, b| {
            barycenters
                .get(a)
                .unwrap_or(&0.0)
                .partial_cmp(barycenters.get(b).unwrap_or(&0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    fn run_barycenter_passes(
        layers: &mut HashMap<i32, Vec<NodeIndex>>,
        sorted_ranks: &[i32],
        layer_coords: &mut HashMap<NodeIndex, f32>,
        relations: &RootRelations,
        barycenter_spacing: f32,
    ) {
        for _ in 0..2 {
            for &rank in sorted_ranks.iter().skip(1) {
                if let Some(layer_nodes) = layers.get_mut(&rank) {
                    Self::order_layer_by_barycenter(layer_nodes, layer_coords, &relations.incoming);
                    for (j, &node_idx) in layer_nodes.iter().enumerate() {
                        layer_coords.insert(node_idx, j as f32 * barycenter_spacing);
                    }
                }
            }

            for i in (0..sorted_ranks.len().saturating_sub(1)).rev() {
                let rank = sorted_ranks[i];
                if let Some(layer_nodes) = layers.get_mut(&rank) {
                    Self::order_layer_by_barycenter(layer_nodes, layer_coords, &relations.outgoing);
                    for (j, &node_idx) in layer_nodes.iter().enumerate() {
                        layer_coords.insert(node_idx, j as f32 * barycenter_spacing);
                    }
                }
            }
        }
    }

    fn child_index(model: &GraphModel, child_id: NodeId) -> Option<NodeIndex> {
        let child_idx = model.node_map.get(&child_id).copied();
        if child_idx.is_none() {
            tracing::warn!("Missing node in node_map: {:?}", child_id);
        }
        child_idx
    }

    fn compute_subtree_size(
        &self,
        model: &GraphModel,
        sizes: &mut HashMap<NodeIndex, Vec2>,
        node_idx: NodeIndex,
        depth: u32,
    ) -> Vec2 {
        // Prevent stack overflow from deeply nested or circular graphs.
        if depth > Self::MAX_NESTING_DEPTH {
            tracing::warn!(
                "Maximum nesting depth ({}) exceeded for node {:?}, using default size",
                Self::MAX_NESTING_DEPTH,
                model.graph[node_idx].id
            );
            let fallback = model.graph[node_idx].size;
            sizes.insert(node_idx, fallback);
            return fallback;
        }

        if let Some(&size) = sizes.get(&node_idx) {
            return size;
        }

        let node = &model.graph[node_idx];

        if !node.expanded || node.children.is_empty() {
            sizes.insert(node_idx, node.size);
            return node.size;
        }

        match node.group_layout {
            GroupLayout::LIST => {
                let mut current_y = 30.0 + self.inner_padding;
                let mut max_width = node.size.x;

                for &child_id in &node.children {
                    let child_size = if let Some(child_idx) = Self::child_index(model, child_id) {
                        self.compute_subtree_size(model, sizes, child_idx, depth + 1)
                    } else {
                        Self::default_node_size()
                    };
                    current_y += child_size.y + self.child_spacing;
                    max_width = max_width.max(child_size.x + 2.0 * self.inner_padding);
                }

                let final_size =
                    Vec2::new(max_width, (current_y + self.inner_padding).max(node.size.y));
                sizes.insert(node_idx, final_size);
                final_size
            }
            GroupLayout::GRID => {
                let child_count = node.children.len();
                let cols = (child_count as f32).sqrt().ceil() as usize;

                let mut current_x = self.inner_padding;
                let mut current_y = 30.0 + self.inner_padding;
                let mut row_max_height = 0.0;
                let mut content_width: f32 = 0.0;

                for (i, &child_id) in node.children.iter().enumerate() {
                    if i > 0 && i % cols == 0 {
                        current_x = self.inner_padding;
                        current_y += row_max_height + self.child_spacing;
                        row_max_height = 0.0;
                    }

                    let child_size = if let Some(child_idx) = Self::child_index(model, child_id) {
                        self.compute_subtree_size(model, sizes, child_idx, depth + 1)
                    } else {
                        Self::default_node_size()
                    };

                    current_x += child_size.x + self.child_spacing;
                    row_max_height = row_max_height.max(child_size.y);
                    content_width = content_width.max(current_x);
                }

                // Account for last row height.
                current_y += row_max_height;

                let final_size = Vec2::new(
                    content_width.max(node.size.x) + self.inner_padding,
                    (current_y + self.inner_padding).max(node.size.y),
                );
                sizes.insert(node_idx, final_size);
                final_size
            }
        }
    }

    fn precompute_sizes(
        &self,
        model: &GraphModel,
        root_nodes: &[NodeIndex],
    ) -> (HashMap<NodeIndex, Vec2>, HashMap<NodeIndex, Vec2>) {
        let mut per_root: Vec<(NodeIndex, Vec<(NodeIndex, Vec2)>, Vec2)> = root_nodes
            .par_iter()
            .map(|&root_idx| {
                let mut local_sizes = HashMap::new();
                let root_size = self.compute_subtree_size(model, &mut local_sizes, root_idx, 0);
                let entries = local_sizes.into_iter().collect::<Vec<_>>();
                (root_idx, entries, root_size)
            })
            .collect();

        // Deterministic merge order.
        per_root.sort_by_key(|(root, _, _)| *root);

        let mut sizes = HashMap::with_capacity(model.graph.node_count());
        let mut root_sizes = HashMap::with_capacity(root_nodes.len());
        for (root_idx, entries, root_size) in per_root {
            root_sizes.insert(root_idx, root_size);
            for (node_idx, size) in entries {
                sizes.entry(node_idx).or_insert(size);
            }
        }
        (sizes, root_sizes)
    }

    fn place_subtree(
        &self,
        model: &GraphModel,
        node_idx: NodeIndex,
        x: f32,
        y: f32,
        positions: &mut HashMap<NodeIndex, (f32, f32)>,
        sizes: &HashMap<NodeIndex, Vec2>,
    ) {
        positions.insert(node_idx, (x, y));

        let node = &model.graph[node_idx];
        if !node.expanded || node.children.is_empty() {
            return;
        }

        let start_y = y + 30.0 + self.inner_padding; // Header height + padding

        match node.group_layout {
            GroupLayout::LIST => {
                let mut current_y = start_y;

                for &child_id in &node.children {
                    let Some(child_idx) = Self::child_index(model, child_id) else {
                        continue;
                    };
                    self.place_subtree(
                        model,
                        child_idx,
                        x + self.inner_padding,
                        current_y,
                        positions,
                        sizes,
                    );

                    let child_size = sizes
                        .get(&child_idx)
                        .copied()
                        .unwrap_or_else(Self::default_node_size);
                    current_y += child_size.y + self.child_spacing;
                }
            }
            GroupLayout::GRID => {
                let child_count = node.children.len();
                let cols = (child_count as f32).sqrt().ceil() as usize;

                let mut current_x = x + self.inner_padding;
                let mut current_y = start_y;
                let mut row_max_height = 0.0;

                for (i, &child_id) in node.children.iter().enumerate() {
                    if i > 0 && i % cols == 0 {
                        current_x = x + self.inner_padding;
                        current_y += row_max_height + self.child_spacing;
                        row_max_height = 0.0;
                    }

                    let Some(child_idx) = Self::child_index(model, child_id) else {
                        continue;
                    };
                    self.place_subtree(model, child_idx, current_x, current_y, positions, sizes);
                    let child_size = sizes
                        .get(&child_idx)
                        .copied()
                        .unwrap_or_else(Self::default_node_size);

                    current_x += child_size.x + self.child_spacing;
                    row_max_height = row_max_height.max(child_size.y);
                }
            }
        }
    }

    fn layer_extent(
        &self,
        layer_nodes: &[NodeIndex],
        root_sizes: &HashMap<NodeIndex, Vec2>,
        node_spacing: f32,
    ) -> f32 {
        let base_extent = match self.direction {
            LayoutDirection::Vertical => layer_nodes
                .iter()
                .map(|idx| {
                    root_sizes
                        .get(idx)
                        .copied()
                        .unwrap_or_else(Self::default_node_size)
                        .x
                })
                .sum::<f32>(),
            LayoutDirection::Horizontal => layer_nodes
                .iter()
                .map(|idx| {
                    root_sizes
                        .get(idx)
                        .copied()
                        .unwrap_or_else(Self::default_node_size)
                        .y
                })
                .sum::<f32>(),
        };

        base_extent + (layer_nodes.len().saturating_sub(1) as f32) * node_spacing
    }

    #[allow(clippy::too_many_arguments)]
    fn place_roots_in_layers(
        &self,
        model: &GraphModel,
        layers: &HashMap<i32, Vec<NodeIndex>>,
        sorted_ranks: &[i32],
        root_sizes: &HashMap<NodeIndex, Vec2>,
        sizes: &HashMap<NodeIndex, Vec2>,
        positions: &mut HashMap<NodeIndex, (f32, f32)>,
        layer_spacing: f32,
        node_spacing: f32,
    ) {
        for rank in sorted_ranks {
            let Some(layer_nodes) = layers.get(rank) else {
                continue;
            };
            let extent = self.layer_extent(layer_nodes, root_sizes, node_spacing);
            let mut current_offset = -extent / 2.0;
            let rank_pos = *rank as f32 * layer_spacing;

            for &node_idx in layer_nodes {
                let root_size = root_sizes
                    .get(&node_idx)
                    .copied()
                    .unwrap_or_else(Self::default_node_size);
                match self.direction {
                    LayoutDirection::Vertical => {
                        self.place_subtree(
                            model,
                            node_idx,
                            current_offset,
                            rank_pos,
                            positions,
                            sizes,
                        );
                        current_offset += root_size.x + node_spacing;
                    }
                    LayoutDirection::Horizontal => {
                        self.place_subtree(
                            model,
                            node_idx,
                            rank_pos,
                            current_offset,
                            positions,
                            sizes,
                        );
                        current_offset += root_size.y + node_spacing;
                    }
                }
            }
        }
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
        let damping = 0.3;

        let mut node_indices: Vec<NodeIndex> = positions.keys().copied().collect();
        // Sort indices to ensure deterministic iteration order for force calculation.
        node_indices.sort();
        if node_indices.is_empty() {
            return;
        }

        let node_ids: Vec<NodeId> = node_indices
            .iter()
            .map(|&idx| model.graph[idx].id)
            .collect();
        let parents: Vec<Option<NodeId>> = node_indices
            .iter()
            .map(|&idx| model.graph[idx].parent)
            .collect();
        let node_sizes: Vec<Vec2> = node_indices
            .iter()
            .map(|&idx| {
                sizes
                    .get(&idx)
                    .copied()
                    .unwrap_or_else(Self::default_node_size)
            })
            .collect();

        for _ in 0..iterations {
            let mut forces_x = vec![0.0f32; node_indices.len()];

            // Calculate repulsion forces between all node pairs.
            for i in 0..node_indices.len() {
                for j in (i + 1)..node_indices.len() {
                    // Skip if same hierarchy (parent-child).
                    if parents[i] == Some(node_ids[j]) || parents[j] == Some(node_ids[i]) {
                        continue;
                    }

                    let a = node_indices[i];
                    let b = node_indices[j];
                    let (ax, ay) = positions[&a];
                    let (bx, by) = positions[&b];
                    let a_size = node_sizes[i];
                    let b_size = node_sizes[j];

                    // Use center positions.
                    let acx = ax + a_size.x / 2.0;
                    let acy = ay + a_size.y / 2.0;
                    let bcx = bx + b_size.x / 2.0;
                    let bcy = by + b_size.y / 2.0;

                    let dx = acx - bcx;
                    let dy = acy - bcy;
                    let dist = (dx * dx + dy * dy).sqrt().max(min_distance);

                    // Only apply horizontal repulsion to avoid breaking layer structure.
                    let force = repulsion_strength / (dist * dist);
                    let fx = if dx.abs() > 0.01 {
                        force * dx.signum()
                    } else {
                        0.0
                    };

                    forces_x[i] += fx;
                    forces_x[j] -= fx;
                }
            }

            // Apply forces with damping.
            for (i, &node_idx) in node_indices.iter().enumerate() {
                if let Some(pos) = positions.get_mut(&node_idx)
                    && let LayoutDirection::Vertical = self.direction
                {
                    pos.0 += forces_x[i] * damping;
                }
            }
        }
    }
}

impl Layouter for NestingLayouter {
    fn execute(
        &self,
        model: &GraphModel,
    ) -> (HashMap<NodeIndex, (f32, f32)>, HashMap<NodeIndex, Vec2>) {
        let mut positions = HashMap::new();
        let root_nodes = Self::root_nodes(model);

        if root_nodes.is_empty() {
            return (positions, HashMap::new());
        }

        let node_roots = Self::build_node_roots(model);
        let relations = Self::build_root_relations(model, &node_roots);
        let ranks = Self::assign_root_ranks(&root_nodes, &relations);
        let mut layers = Self::build_layers(model, &ranks);
        let sorted_ranks = Self::sorted_ranks(&layers);
        let (barycenter_spacing, layer_spacing, node_spacing) =
            Self::spacing_for_root_count(root_nodes.len());
        let mut layer_coords =
            Self::initialize_layer_coords(&layers, &sorted_ranks, barycenter_spacing);
        Self::run_barycenter_passes(
            &mut layers,
            &sorted_ranks,
            &mut layer_coords,
            &relations,
            barycenter_spacing,
        );

        let (sizes, root_sizes) = self.precompute_sizes(model, &root_nodes);
        self.place_roots_in_layers(
            model,
            &layers,
            &sorted_ranks,
            &root_sizes,
            &sizes,
            &mut positions,
            layer_spacing,
            node_spacing,
        );

        self.apply_force_directed(&mut positions, &sizes, model, 5);

        (positions, sizes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_core::{Edge, EdgeKind, Node, NodeId, NodeKind};

    fn add_node(model: &mut GraphModel, id: i64, name: &str) {
        model.add_node(Node {
            id: NodeId(id),
            kind: NodeKind::FUNCTION,
            serialized_name: name.to_string(),
            ..Default::default()
        });
    }

    fn add_edge(model: &mut GraphModel, id: i64, source: i64, target: i64) {
        model.add_edge(Edge {
            id: codestory_core::EdgeId(id),
            source: NodeId(source),
            target: NodeId(target),
            kind: EdgeKind::CALL,
            ..Default::default()
        });
    }

    #[test]
    fn test_nesting_layout_returns_positions_and_sizes() {
        let mut model = GraphModel::new();
        add_node(&mut model, 1, "Root");
        add_node(&mut model, 2, "Child");

        let root_idx = *model.node_map.get(&NodeId(1)).unwrap();
        let child_idx = *model.node_map.get(&NodeId(2)).unwrap();

        model.graph[root_idx].expanded = true;
        model.graph[root_idx].children.push(NodeId(2));
        model.graph[child_idx].parent = Some(NodeId(1));

        let layouter = NestingLayouter {
            inner_padding: NestingLayouter::DEFAULT_INNER_PADDING,
            child_spacing: NestingLayouter::DEFAULT_CHILD_SPACING,
            direction: LayoutDirection::Vertical,
        };

        let (positions, sizes) = layouter.execute(&model);

        assert_eq!(positions.len(), 2);
        assert_eq!(sizes.len(), 2);
        assert!(sizes[&root_idx].y > sizes[&child_idx].y);
    }

    #[test]
    fn test_nesting_layout_direction_changes_primary_axis() {
        let mut model = GraphModel::new();
        add_node(&mut model, 1, "A");
        add_node(&mut model, 2, "B");
        add_edge(&mut model, 1, 1, 2);

        let vertical = NestingLayouter {
            inner_padding: NestingLayouter::DEFAULT_INNER_PADDING,
            child_spacing: NestingLayouter::DEFAULT_CHILD_SPACING,
            direction: LayoutDirection::Vertical,
        };
        let horizontal = NestingLayouter {
            inner_padding: NestingLayouter::DEFAULT_INNER_PADDING,
            child_spacing: NestingLayouter::DEFAULT_CHILD_SPACING,
            direction: LayoutDirection::Horizontal,
        };

        let (v_pos, _) = vertical.execute(&model);
        let (h_pos, _) = horizontal.execute(&model);

        let a = *model.node_map.get(&NodeId(1)).unwrap();
        let b = *model.node_map.get(&NodeId(2)).unwrap();

        assert!((v_pos[&b].1 - v_pos[&a].1).abs() > 0.1);
        assert!((h_pos[&b].0 - h_pos[&a].0).abs() > 0.1);
    }

    #[test]
    fn test_edge_bundler_groups_parallel_edges() {
        let mut model = GraphModel::new();
        add_node(&mut model, 1, "A");
        add_node(&mut model, 2, "B");

        model.add_edge(Edge {
            id: codestory_core::EdgeId(1),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::CALL,
            ..Default::default()
        });
        model.add_edge(Edge {
            id: codestory_core::EdgeId(2),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::USAGE,
            ..Default::default()
        });

        let bundles = EdgeBundler::bundle_edges(&model);

        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].len(), 2);
    }
}
