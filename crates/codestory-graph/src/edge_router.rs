use crate::Vec2;
use crate::uml_types::{BundleData, Rect};
use codestory_core::{EdgeId, EdgeKind, NodeId};
use std::collections::HashMap;

/// Minimum number of edges between the same source/target to trigger bundling.
///
/// **Validates: Requirements 14.1, Property 10: Edge Bundling Threshold**
pub const BUNDLE_THRESHOLD: usize = 3;

/// Descriptor for an edge, used as input to `bundle_edges()`.
#[derive(Debug, Clone)]
pub struct EdgeDescriptor {
    pub id: EdgeId,
    pub source_node: NodeId,
    pub target_node: NodeId,
    pub kind: EdgeKind,
    pub source_label: String,
    pub target_label: String,
}

/// A group of edges that have been bundled together.
#[derive(Debug, Clone)]
pub struct EdgeBundleGroup {
    pub source_node: NodeId,
    pub target_node: NodeId,
    pub edge_ids: Vec<EdgeId>,
    pub data: BundleData,
}

/// Result of edge bundling: some edges are bundled, others remain individual.
#[derive(Debug, Clone)]
pub struct BundleResult {
    /// Edge groups that meet the bundling threshold (3+).
    pub bundles: Vec<EdgeBundleGroup>,
    /// Individual edges that did not meet the threshold.
    pub unbundled: Vec<EdgeDescriptor>,
}

/// A cubic bezier curve segment defined by four control points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicBezier {
    pub start: Vec2,
    pub control1: Vec2,
    pub control2: Vec2,
    pub end: Vec2,
}

impl CubicBezier {
    /// Sample the curve at parameter t [0, 1]
    pub fn sample(&self, t: f32) -> Vec2 {
        let t2 = t * t;
        let t3 = t2 * t;
        let mt = 1.0 - t;
        let mt2 = mt * mt;
        let mt3 = mt2 * mt;

        let x = self.start.x * mt3
            + 3.0 * self.control1.x * mt2 * t
            + 3.0 * self.control2.x * mt * t2
            + self.end.x * t3;
        let y = self.start.y * mt3
            + 3.0 * self.control1.y * mt2 * t
            + 3.0 * self.control2.y * mt * t2
            + self.end.y * t3;

        Vec2::new(x, y)
    }

    /// Compute the minimum distance from a point to this bezier curve.
    ///
    /// Uses uniform sampling along the curve to find the closest point.
    /// The `num_samples` parameter controls accuracy (higher = more precise but slower).
    ///
    /// # Arguments
    /// * `point` - The point to test against
    /// * `num_samples` - Number of samples along the curve (typically 20-50)
    ///
    /// # Returns
    /// The minimum distance from the point to any sampled point on the curve.
    ///
    /// **Validates: Requirements 8.1, 11.2**
    pub fn point_distance(&self, point: Vec2, num_samples: usize) -> f32 {
        let mut min_dist_sq = f32::INFINITY;
        let samples = num_samples.max(2);

        for i in 0..=samples {
            let t = i as f32 / samples as f32;
            let curve_point = self.sample(t);
            let dx = curve_point.x - point.x;
            let dy = curve_point.y - point.y;
            let dist_sq = dx * dx + dy * dy;
            if dist_sq < min_dist_sq {
                min_dist_sq = dist_sq;
            }
        }

        min_dist_sq.sqrt()
    }
}

/// Router for calculating edge paths between nodes and members.
#[derive(Debug, Clone, Copy)]
pub struct EdgeRouter {
    /// Margin around nodes to avoid routing through
    pub node_margin: f32,
    /// Curvature factor for bezier control points (usually related to distance)
    pub curvature: f32,
}

impl Default for EdgeRouter {
    fn default() -> Self {
        Self {
            node_margin: 20.0,
            curvature: 0.5,
        }
    }
}

impl EdgeRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Calculate a cubic bezier route between two rectangles.
    ///
    /// # Arguments
    /// * `source_rect` - The bounding box of the source node or member.
    /// * `target_rect` - The bounding box of the target node or member.
    ///
    /// Returns a `CubicBezier` curve.
    pub fn route_edge(&self, source_rect: Rect, target_rect: Rect) -> CubicBezier {
        let start = self.calculate_anchor(source_rect, target_rect.center());
        let end = self.calculate_anchor(target_rect, source_rect.center());

        self.calculate_curve(start, end, source_rect, target_rect)
    }

    /// Calculate curve points giving start and end positions and their bounding boxes
    /// to determine control point direction.
    fn calculate_curve(
        &self,
        start: Vec2,
        end: Vec2,
        source_rect: Rect,
        target_rect: Rect,
    ) -> CubicBezier {
        // Prevent control points from scaling unbounded with edge length.
        // If they do, edges can "swing" far outside the viewport and look comically long.
        const MAX_CONTROL_LEN: f32 = 260.0;

        // Basic vector math helper since Vec2 might not implement ops
        let sub = |a: Vec2, b: Vec2| Vec2::new(a.x - b.x, a.y - b.y);
        let add = |a: Vec2, b: Vec2| Vec2::new(a.x + b.x, a.y + b.y);
        let mul = |v: Vec2, s: f32| Vec2::new(v.x * s, v.y * s);

        let delta = sub(end, start);

        // Use a directionally-biased "distance" instead of full Euclidean distance.
        // This keeps curves stable when nodes are far apart vertically but close horizontally,
        // and also reduces the chance of overshooting control points.
        let dx = delta.x.abs();
        let dy = delta.y.abs();
        let primary_dist = dx.max(dy * 0.5);

        let control_dist = primary_dist * self.curvature;

        // Determine face directions based on anchor points relative to rect centers
        let start_dir = self.get_normal_direction(start, source_rect);
        let end_dir = self.get_normal_direction(end, target_rect);

        let mut curve_len = if primary_dist < self.node_margin * 2.0 {
            // For very close nodes, keep control points close so we don't create loops.
            control_dist
        } else {
            control_dist.max(self.node_margin)
        };
        if curve_len.is_finite() {
            curve_len = curve_len.min(MAX_CONTROL_LEN);
        } else {
            curve_len = self.node_margin.min(MAX_CONTROL_LEN);
        }

        // control1 = start + start_dir * curve_len
        let control1 = add(start, mul(start_dir, curve_len));
        // control2 = end + end_dir * curve_len
        let control2 = add(end, mul(end_dir, curve_len));

        CubicBezier {
            start,
            control1,
            control2,
            end,
        }
    }

    /// Calculate the best anchor point on the border of `rect` to connect to `target_center`.
    pub fn calculate_anchor(&self, rect: Rect, target_center: Vec2) -> Vec2 {
        let center = rect.center();

        // Basic vector math helper
        let sub = |a: Vec2, b: Vec2| Vec2::new(a.x - b.x, a.y - b.y);
        let add = |a: Vec2, b: Vec2| Vec2::new(a.x + b.x, a.y + b.y);
        let mul = |v: Vec2, s: f32| Vec2::new(v.x * s, v.y * s);
        let len_sq = |v: Vec2| v.x * v.x + v.y * v.y;

        let vec = sub(target_center, center);

        if len_sq(vec) < 1.0 {
            return center;
        }

        // Ray casting from center to target, intersecting rect bounds.
        // Rect sides: x=left, x=right, y=top, y=bottom

        let mut t_min = f32::INFINITY;

        // Helper to check standard intersection
        let check_t = |t: f32, start: f32, dir: f32, min: f32, max: f32| -> Option<f32> {
            if t > 0.0 {
                let pos = start + t * dir;
                if pos >= min && pos <= max {
                    return Some(t);
                }
            }
            None
        };

        // Check X sides (left and right)
        if vec.x.abs() > 0.001 {
            let t_left = (rect.min.x - center.x) / vec.x;
            if let Some(t) = check_t(t_left, center.y, vec.y, rect.min.y, rect.max.y) {
                t_min = t_min.min(t);
            }

            let t_right = (rect.max.x - center.x) / vec.x;
            if let Some(t) = check_t(t_right, center.y, vec.y, rect.min.y, rect.max.y) {
                t_min = t_min.min(t);
            }
        }

        // Check Y sides (top and bottom)
        if vec.y.abs() > 0.001 {
            let t_top = (rect.min.y - center.y) / vec.y;
            if let Some(t) = check_t(t_top, center.x, vec.x, rect.min.x, rect.max.x) {
                t_min = t_min.min(t);
            }

            let t_bottom = (rect.max.y - center.y) / vec.y;
            if let Some(t) = check_t(t_bottom, center.x, vec.x, rect.min.x, rect.max.x) {
                t_min = t_min.min(t);
            }
        }

        if t_min.is_infinite() {
            return center; // Should not happen ideally
        }

        add(center, mul(vec, t_min))
    }

    /// Bundle edges that share the same source and target nodes.
    ///
    /// Groups edges by their (source_node, target_node) pair. When 3 or more edges
    /// share the same endpoints, they are bundled into a single `EdgeBundle`.
    /// Edges below the threshold are returned as-is (unbundled).
    ///
    /// # Arguments
    /// * `edges` - Slice of edge descriptors to consider for bundling.
    ///
    /// # Returns
    /// A `BundleResult` containing both bundled groups and unbundled individual edges.
    ///
    /// **Validates: Requirements 3.5, 14.1 and Property 10: Edge Bundling Threshold**
    pub fn bundle_edges(&self, edges: &[EdgeDescriptor]) -> BundleResult {
        // Group edges by (source_node, target_node)
        let mut groups: HashMap<(NodeId, NodeId), Vec<&EdgeDescriptor>> = HashMap::new();
        for edge in edges {
            groups
                .entry((edge.source_node, edge.target_node))
                .or_default()
                .push(edge);
        }

        let mut bundles = Vec::new();
        let mut unbundled = Vec::new();

        for ((source_node, target_node), group) in groups {
            if group.len() >= BUNDLE_THRESHOLD {
                // Create a bundle
                let edge_ids: Vec<EdgeId> = group.iter().map(|e| e.id).collect();
                let edge_kinds: Vec<EdgeKind> = group.iter().map(|e| e.kind).collect();
                let relationships: Vec<(String, String, EdgeKind)> = group
                    .iter()
                    .map(|e| (e.source_label.clone(), e.target_label.clone(), e.kind))
                    .collect();

                let bundle_data = BundleData::new(edge_ids.clone(), edge_kinds, relationships);

                bundles.push(EdgeBundleGroup {
                    source_node,
                    target_node,
                    edge_ids,
                    data: bundle_data,
                });
            } else {
                // Keep as individual edges
                for edge in group {
                    unbundled.push(edge.clone());
                }
            }
        }

        BundleResult { bundles, unbundled }
    }

    /// Get approximate normal direction from rect center to point on its border
    fn get_normal_direction(&self, point: Vec2, rect: Rect) -> Vec2 {
        // Avoid hard epsilons against border equality: we often get points that are
        // microscopically inside/outside due to transform + float math, and a strict check
        // can pick the wrong direction (which then produces very long curves).
        let dl = (point.x - rect.min.x).abs();
        let dr = (point.x - rect.max.x).abs();
        let dt = (point.y - rect.min.y).abs();
        let db = (point.y - rect.max.y).abs();

        if dl <= dr && dl <= dt && dl <= db {
            return Vec2::new(-1.0, 0.0);
        }
        if dr <= dl && dr <= dt && dr <= db {
            return Vec2::new(1.0, 0.0);
        }
        if dt <= dl && dt <= dr && dt <= db {
            return Vec2::new(0.0, -1.0);
        }
        Vec2::new(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vec2;
    use crate::uml_types::Rect;
    use proptest::prelude::*;

    // Helper to check if a point is approximately on a rect border
    fn is_on_border(p: Vec2, r: Rect, epsilon: f32) -> bool {
        (p.x - r.min.x).abs() < epsilon && p.y >= r.min.y - epsilon && p.y <= r.max.y + epsilon
            || (p.x - r.max.x).abs() < epsilon
                && p.y >= r.min.y - epsilon
                && p.y <= r.max.y + epsilon
            || (p.y - r.min.y).abs() < epsilon
                && p.x >= r.min.x - epsilon
                && p.x <= r.max.x + epsilon
            || (p.y - r.max.y).abs() < epsilon
                && p.x >= r.min.x - epsilon
                && p.x <= r.max.x + epsilon
    }

    // Custom distance impl
    fn distance(a: Vec2, b: Vec2) -> f32 {
        ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
    }

    fn rect_strategy() -> impl Strategy<Value = Rect> {
        (
            0.0f32..1000.0,
            0.0f32..1000.0,
            10.0f32..100.0,
            10.0f32..100.0,
        )
            .prop_map(|(x, y, w, h)| Rect::from_pos_size(Vec2::new(x, y), Vec2::new(w, h)))
    }

    proptest! {
        /// Property 9: Bezier structure continuity
        #[test]
        fn prop_bezier_continuity(
            source_rect in rect_strategy(),
            target_rect in rect_strategy()
        ) {
            prop_assume!(distance(source_rect.center(), target_rect.center()) > 1.0);

            let router = EdgeRouter::new();
            let curve = router.route_edge(source_rect, target_rect);

            let expected_start = router.calculate_anchor(source_rect, target_rect.center());
            let expected_end = router.calculate_anchor(target_rect, source_rect.center());

            let start_dist = distance(curve.start, expected_start);
            let end_dist = distance(curve.end, expected_end);

            prop_assert!(start_dist < 0.001, "Curve start should match source anchor. Got {:?}, expected {:?}", curve.start, expected_start);
            prop_assert!(end_dist < 0.001, "Curve end should match target anchor. Got {:?}, expected {:?}", curve.end, expected_end);
        }

        /// Property 11: Edge Anchoring
        #[test]
        fn prop_anchor_positioning(
            rect in rect_strategy(),
            target_x in -1000.0f32..2000.0,
            target_y in -1000.0f32..2000.0
        ) {
            let target = Vec2::new(target_x, target_y);
            let router = EdgeRouter::new();

            // Check if point is inside
            let is_inside = target.x >= rect.min.x && target.x <= rect.max.x &&
                           target.y >= rect.min.y && target.y <= rect.max.y;

            if !is_inside {
                 let anchor = router.calculate_anchor(rect, target);
                 prop_assert!(is_on_border(anchor, rect, 0.1), "Anchor {:?} should be on border of {:?}", anchor, rect);
            }
        }

        /// Property 12: Collapsed Node Routing
        #[test]
        fn prop_collapsed_node_routing(
            node_rect in rect_strategy(),
            target_pos_x in 0.0f32..1000.0,
            target_pos_y in 0.0f32..1000.0
        ) {
             let target_pos = Vec2::new(target_pos_x, target_pos_y);

             // Member inside
             let member_rect = Rect::from_pos_size(
                 Vec2::new(node_rect.min.x + 5.0, node_rect.min.y + 5.0),
                 Vec2::new(20.0, 10.0)
             );

             // Ensure member is inside
             prop_assume!(member_rect.max.x < node_rect.max.x && member_rect.max.y < node_rect.max.y);

             // Ensure target is outside node
             let is_inside_node = target_pos.x >= node_rect.min.x && target_pos.x <= node_rect.max.x &&
                                  target_pos.y >= node_rect.min.y && target_pos.y <= node_rect.max.y;
             prop_assume!(!is_inside_node);

             let router = EdgeRouter::new();

             // Expanded -> Member anchor
             let member_anchor = router.calculate_anchor(member_rect, target_pos);
             prop_assert!(is_on_border(member_anchor, member_rect, 0.1));

             // Collapsed -> Node anchor
             let node_anchor = router.calculate_anchor(node_rect, target_pos);
             prop_assert!(is_on_border(node_anchor, node_rect, 0.1));

             if !is_on_border(member_anchor, node_rect, 1.0) {
                 prop_assert!(distance(member_anchor, node_anchor) > 0.001);
             }
        }
    }

    /// Strategy to generate EdgeKind values
    fn edge_kind_strategy() -> impl Strategy<Value = EdgeKind> {
        prop_oneof![
            Just(EdgeKind::MEMBER),
            Just(EdgeKind::TYPE_USAGE),
            Just(EdgeKind::USAGE),
            Just(EdgeKind::CALL),
            Just(EdgeKind::INHERITANCE),
            Just(EdgeKind::OVERRIDE),
            Just(EdgeKind::TYPE_ARGUMENT),
            Just(EdgeKind::TEMPLATE_SPECIALIZATION),
            Just(EdgeKind::INCLUDE),
            Just(EdgeKind::IMPORT),
            Just(EdgeKind::MACRO_USAGE),
            Just(EdgeKind::ANNOTATION_USAGE),
            Just(EdgeKind::UNKNOWN),
        ]
    }

    /// Strategy to generate a vector of EdgeDescriptors between two fixed nodes
    fn edge_descriptors_strategy(
        min_count: usize,
        max_count: usize,
    ) -> impl Strategy<Value = Vec<EdgeDescriptor>> {
        proptest::collection::vec(edge_kind_strategy(), min_count..=max_count).prop_map(|kinds| {
            kinds
                .into_iter()
                .enumerate()
                .map(|(i, kind)| EdgeDescriptor {
                    id: EdgeId(i as i64 + 1),
                    source_node: NodeId(1),
                    target_node: NodeId(2),
                    kind,
                    source_label: "SourceNode".to_string(),
                    target_label: "TargetNode".to_string(),
                })
                .collect()
        })
    }

    /// Strategy to generate EdgeDescriptors across multiple node pairs
    fn multi_pair_edge_descriptors_strategy() -> impl Strategy<Value = Vec<EdgeDescriptor>> {
        // Generate 1-10 edges for up to 4 node pairs
        (
            proptest::collection::vec(edge_kind_strategy(), 0..=5), // pair (1,2)
            proptest::collection::vec(edge_kind_strategy(), 0..=5), // pair (1,3)
            proptest::collection::vec(edge_kind_strategy(), 0..=5), // pair (2,3)
            proptest::collection::vec(edge_kind_strategy(), 0..=5), // pair (3,4)
        )
            .prop_map(|(p12, p13, p23, p34)| {
                let mut edges = Vec::new();
                let mut id = 1i64;

                let pairs = [
                    (NodeId(1), NodeId(2), p12),
                    (NodeId(1), NodeId(3), p13),
                    (NodeId(2), NodeId(3), p23),
                    (NodeId(3), NodeId(4), p34),
                ];

                for (src, tgt, kinds) in pairs {
                    for kind in kinds {
                        edges.push(EdgeDescriptor {
                            id: EdgeId(id),
                            source_node: src,
                            target_node: tgt,
                            kind,
                            source_label: format!("Node({})", src.0),
                            target_label: format!("Node({})", tgt.0),
                        });
                        id += 1;
                    }
                }

                edges
            })
    }

    proptest! {
        /// **Validates: Property 10: Edge Bundling Threshold**
        ///
        /// For any set of edges, bundle_edges() SHALL only create bundles when
        /// 3 or more edges share the same source and target nodes.
        /// Groups with fewer than 3 edges MUST remain unbundled.
        #[test]
        fn prop_edge_bundling_threshold(
            edges in multi_pair_edge_descriptors_strategy()
        ) {
            let router = EdgeRouter::new();
            let result = router.bundle_edges(&edges);

            // Count edges per (source, target) pair
            let mut pair_counts: HashMap<(NodeId, NodeId), usize> = HashMap::new();
            for edge in &edges {
                *pair_counts.entry((edge.source_node, edge.target_node)).or_default() += 1;
            }

            // Property: Every bundle must have >= BUNDLE_THRESHOLD edges
            for bundle in &result.bundles {
                prop_assert!(
                    bundle.edge_ids.len() >= BUNDLE_THRESHOLD,
                    "Bundle ({:?}, {:?}) has {} edges, below threshold {}",
                    bundle.source_node, bundle.target_node,
                    bundle.edge_ids.len(), BUNDLE_THRESHOLD
                );
            }

            // Property: No unbundled edge should belong to a pair with >= BUNDLE_THRESHOLD edges
            for edge in &result.unbundled {
                let pair_count = pair_counts[&(edge.source_node, edge.target_node)];
                prop_assert!(
                    pair_count < BUNDLE_THRESHOLD,
                    "Unbundled edge {:?} belongs to pair ({:?}, {:?}) with {} edges, should be bundled",
                    edge.id, edge.source_node, edge.target_node, pair_count
                );
            }

            // Property: Total edge count is preserved
            let total_bundled: usize = result.bundles.iter().map(|b| b.edge_ids.len()).sum();
            let total_unbundled = result.unbundled.len();
            prop_assert_eq!(
                total_bundled + total_unbundled,
                edges.len(),
                "Total edges must be preserved: {} bundled + {} unbundled != {} input",
                total_bundled, total_unbundled, edges.len()
            );
        }

        /// **Validates: Property 30: Edge Bundle Count Badge**
        ///
        /// For any bundled edge group with N edges, the BundleData SHALL report
        /// edge_count() == N and the count must match the number of edge_ids.
        #[test]
        fn prop_bundle_count_badge(
            edges in edge_descriptors_strategy(BUNDLE_THRESHOLD, 20)
        ) {
            let router = EdgeRouter::new();
            let result = router.bundle_edges(&edges);

            // Since all edges go between (1,2), we should get exactly one bundle
            prop_assert_eq!(
                result.bundles.len(), 1,
                "All edges share same endpoints, should produce exactly 1 bundle"
            );

            let bundle = &result.bundles[0];

            // Property: edge_count matches the number of input edges
            prop_assert_eq!(
                bundle.data.edge_count(), edges.len(),
                "Bundle count badge should show {} edges, got {}",
                edges.len(), bundle.data.edge_count()
            );

            // Property: edge_ids length matches
            prop_assert_eq!(
                bundle.edge_ids.len(), edges.len(),
                "Bundle edge_ids length should match input edge count"
            );

            // Property: relationships count matches
            prop_assert_eq!(
                bundle.data.relationships.len(), edges.len(),
                "Bundle relationships count should match input edge count"
            );

            // Property: All input edge IDs are present in the bundle
            for edge in &edges {
                prop_assert!(
                    bundle.edge_ids.contains(&edge.id),
                    "Edge {:?} should be in bundle", edge.id
                );
            }
        }

        /// **Validates: Property 31: Edge Bundle Thickness Scaling**
        ///
        /// For any bundled edge group, the thickness SHALL follow logarithmic scaling:
        /// min(log2(N) + base_width, max_width) where base_width = 1.0 and max_width = 6.0.
        /// Thickness must always be >= base_width and <= max_width.
        #[test]
        fn prop_bundle_thickness_scaling(
            edge_count in BUNDLE_THRESHOLD..100usize
        ) {
            use crate::uml_types::BundleData;

            let thickness = BundleData::calculate_thickness(edge_count);

            // Property: Thickness must be at least base_width (1.0)
            prop_assert!(
                thickness >= 1.0,
                "Thickness {} for count {} should be >= 1.0",
                thickness, edge_count
            );

            // Property: Thickness must not exceed max_width (6.0)
            prop_assert!(
                thickness <= 6.0,
                "Thickness {} for count {} should be <= 6.0",
                thickness, edge_count
            );

            // Property: Thickness should follow log2 formula
            let expected = ((edge_count as f32).log2() + 1.0).min(6.0);
            prop_assert!(
                (thickness - expected).abs() < 0.001,
                "Thickness {} should equal log2({}) + 1.0 = {}, clamped to 6.0",
                thickness, edge_count, expected
            );

            // Property: Monotonicity -- more edges means >= thickness
            if edge_count > BUNDLE_THRESHOLD {
                let prev_thickness = BundleData::calculate_thickness(edge_count - 1);
                prop_assert!(
                    thickness >= prev_thickness,
                    "Thickness should be monotonically non-decreasing: {} (count {}) < {} (count {})",
                    thickness, edge_count, prev_thickness, edge_count - 1
                );
            }
        }

        /// **Validates: Property 32: Edge Bundle Expansion**
        ///
        /// When a bundle is expanded, all individual edges SHALL become visible.
        /// When collapsed, only the single bundled representation is shown.
        /// Toggling expansion state twice SHALL return to original state.
        #[test]
        fn prop_edge_bundle_expansion(
            edges in edge_descriptors_strategy(BUNDLE_THRESHOLD, 20)
        ) {
            let router = EdgeRouter::new();
            let result = router.bundle_edges(&edges);

            prop_assert_eq!(result.bundles.len(), 1, "Should produce exactly 1 bundle");

            let bundle = &result.bundles[0];

            // Property: BundleData starts not expanded
            prop_assert!(
                !bundle.data.is_expanded,
                "Bundle should start not expanded"
            );

            // Property: toggle_expanded flips state
            let mut data = bundle.data.clone();
            data.toggle_expanded();
            prop_assert!(
                data.is_expanded,
                "Bundle should be expanded after toggle"
            );

            // Property: When expanded, all edge_ids should still be present
            prop_assert_eq!(
                data.edge_ids.len(), edges.len(),
                "Expanded bundle should preserve all edge IDs"
            );

            // Property: All edge kinds should still be present
            prop_assert_eq!(
                data.edge_kinds.len(), edges.len(),
                "Expanded bundle should preserve all edge kinds"
            );

            // Property: Double toggle returns to original state
            data.toggle_expanded();
            prop_assert!(
                !data.is_expanded,
                "Bundle should be collapsed after double toggle"
            );

            // Property: set_expanded works correctly
            data.set_expanded(true);
            prop_assert!(data.is_expanded, "set_expanded(true) should expand");
            data.set_expanded(false);
            prop_assert!(!data.is_expanded, "set_expanded(false) should collapse");
        }
    }

    #[test]
    fn test_normal_direction_is_stable_near_borders() {
        let router = EdgeRouter::new();
        let r = Rect::from_pos_size(Vec2::new(10.0, 20.0), Vec2::new(100.0, 60.0));

        // Points that are slightly off due to float error should still map to the expected side.
        let leftish = Vec2::new(r.min.x + 0.0001, r.center().y);
        let rightish = Vec2::new(r.max.x - 0.0001, r.center().y);
        let topish = Vec2::new(r.center().x, r.min.y + 0.0001);
        let bottomish = Vec2::new(r.center().x, r.max.y - 0.0001);

        assert_eq!(router.get_normal_direction(leftish, r), Vec2::new(-1.0, 0.0));
        assert_eq!(router.get_normal_direction(rightish, r), Vec2::new(1.0, 0.0));
        assert_eq!(router.get_normal_direction(topish, r), Vec2::new(0.0, -1.0));
        assert_eq!(router.get_normal_direction(bottomish, r), Vec2::new(0.0, 1.0));
    }
}
