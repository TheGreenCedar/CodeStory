use crate::Vec2;
use crate::edge_router::CubicBezier;
use crate::uml_types::{EdgeRoute, Rect, UmlNode};
use codestory_core::{EdgeId, NodeId};
use std::collections::HashMap;

/// Result of a hit test at a given position.
///
/// Priority order: Member > Node > EdgeBundle > Edge > None
#[derive(Debug, Clone, PartialEq)]
pub enum HitResult {
    /// Nothing was hit at the tested position.
    None,
    /// A container node was hit (but not a specific member).
    Node(NodeId),
    /// A specific member within a container node was hit.
    Member { node_id: NodeId, member_id: NodeId },
    /// A single edge was hit.
    Edge(EdgeId),
    /// An edge bundle was hit (multiple edges grouped together).
    EdgeBundle {
        bundle_id: usize,
        edges: Vec<EdgeId>,
    },
}

/// Information about an edge bundle for hit testing purposes.
#[derive(Debug, Clone)]
pub struct EdgeBundleRegion {
    /// Unique identifier for this bundle.
    pub bundle_id: usize,
    /// All edge IDs in this bundle.
    pub edges: Vec<EdgeId>,
    /// The bezier curve for this bundle (used for distance testing).
    pub curve: CubicBezier,
}

/// Comprehensive hit tester for the graph visualization.
///
/// Maintains spatial information about nodes, members, edges, and edge bundles,
/// and provides hit testing with correct priority ordering:
/// Member > Node > EdgeBundle > Edge > None
///
/// **Validates: Requirements 11.2**
#[derive(Debug, Clone)]
pub struct HitTester {
    /// Bounding rectangles for each container node.
    node_rects: HashMap<NodeId, Rect>,
    /// Bounding rectangles for members within each node.
    /// Outer key is the container node ID, inner key is the member ID.
    member_rects: HashMap<NodeId, HashMap<NodeId, Rect>>,
    /// Bezier curves for each edge, used for distance-based hit testing.
    edge_curves: HashMap<EdgeId, CubicBezier>,
    /// Edge bundle regions for bundled edge hit testing.
    edge_bundle_regions: Vec<EdgeBundleRegion>,
    /// Default tolerance (in pixels) for edge hit testing.
    edge_tolerance: f32,
    /// Number of samples along bezier curves for distance computation.
    bezier_samples: usize,
}

impl Default for HitTester {
    fn default() -> Self {
        Self::new()
    }
}

impl HitTester {
    /// Create a new HitTester with default settings.
    pub fn new() -> Self {
        Self {
            node_rects: HashMap::new(),
            member_rects: HashMap::new(),
            edge_curves: HashMap::new(),
            edge_bundle_regions: Vec::new(),
            edge_tolerance: 8.0,
            bezier_samples: 48,
        }
    }

    /// Create a new HitTester with custom edge tolerance.
    pub fn with_tolerance(tolerance: f32) -> Self {
        Self {
            edge_tolerance: tolerance,
            ..Self::new()
        }
    }

    /// Get the current edge hit tolerance.
    pub fn edge_tolerance(&self) -> f32 {
        self.edge_tolerance
    }

    /// Set the edge hit tolerance.
    pub fn set_edge_tolerance(&mut self, tolerance: f32) {
        self.edge_tolerance = tolerance;
    }

    /// Get the number of bezier samples used for distance computation.
    pub fn bezier_samples(&self) -> usize {
        self.bezier_samples
    }

    /// Set the number of bezier samples.
    pub fn set_bezier_samples(&mut self, samples: usize) {
        self.bezier_samples = samples;
    }

    /// Update hit regions from node data and edge routes.
    ///
    /// Call this after any layout change to refresh the spatial data used for hit testing.
    ///
    /// # Arguments
    /// * `nodes` - Map of all UML nodes with their computed rects and member rects.
    /// * `edges` - Slice of edge routes with computed bezier control points.
    pub fn update(&mut self, nodes: &HashMap<NodeId, UmlNode>, edges: &[EdgeRoute]) {
        self.node_rects.clear();
        self.member_rects.clear();
        self.edge_curves.clear();
        self.edge_bundle_regions.clear();

        // Extract node and member rects from UmlNodes
        for (node_id, node) in nodes {
            self.node_rects.insert(*node_id, node.computed_rect);

            if !node.member_rects.is_empty() {
                self.member_rects
                    .insert(*node_id, node.member_rects.clone());
            }
        }

        // Extract edge curves from edge routes
        for edge in edges {
            let curve = Self::edge_route_to_curve(edge);
            self.edge_curves.insert(edge.id, curve);
        }
    }

    /// Update hit regions with explicit rects (useful when UmlNode data is not available).
    ///
    /// # Arguments
    /// * `node_rects` - Map of node IDs to their bounding rectangles.
    /// * `member_rects` - Map of node IDs to their member rect maps.
    /// * `edges` - Slice of edge routes.
    pub fn update_from_rects(
        &mut self,
        node_rects: HashMap<NodeId, Rect>,
        member_rects: HashMap<NodeId, HashMap<NodeId, Rect>>,
        edges: &[EdgeRoute],
    ) {
        self.node_rects = node_rects;
        self.member_rects = member_rects;
        self.edge_curves.clear();
        self.edge_bundle_regions.clear();

        for edge in edges {
            let curve = Self::edge_route_to_curve(edge);
            self.edge_curves.insert(edge.id, curve);
        }
    }

    /// Add an edge bundle region for hit testing.
    ///
    /// Call this after `update()` to register bundled edges.
    pub fn add_edge_bundle(&mut self, bundle_id: usize, edges: Vec<EdgeId>, curve: CubicBezier) {
        self.edge_bundle_regions.push(EdgeBundleRegion {
            bundle_id,
            edges,
            curve,
        });
    }

    /// Perform a hit test at the given position.
    ///
    /// Returns a `HitResult` indicating what was hit. Priority order is:
    /// 1. Member (highest priority -- most specific)
    /// 2. Node
    /// 3. EdgeBundle
    /// 4. Edge
    /// 5. None (nothing hit)
    ///
    /// **Validates: Requirements 11.2, Property 26**
    pub fn hit_test(&self, pos: Vec2) -> HitResult {
        // Priority 1: Check members first (most specific)
        if let Some((node_id, member_id)) = self.hit_test_member(pos) {
            return HitResult::Member { node_id, member_id };
        }

        // Priority 2: Check nodes
        if let Some(node_id) = self.hit_test_node(pos) {
            return HitResult::Node(node_id);
        }

        // Priority 3: Check edge bundles
        if let Some(region) = self.hit_test_edge_bundle(pos, self.edge_tolerance) {
            return HitResult::EdgeBundle {
                bundle_id: region.bundle_id,
                edges: region.edges.clone(),
            };
        }

        // Priority 4: Check individual edges
        if let Some(edge_id) = self.hit_test_edge(pos, self.edge_tolerance) {
            return HitResult::Edge(edge_id);
        }

        HitResult::None
    }

    /// Test if a position hits any node, returning the node ID.
    pub fn hit_test_node(&self, pos: Vec2) -> Option<NodeId> {
        // If multiple nodes overlap, return the one with the smallest area
        // (most specific / innermost node)
        let mut best: Option<(NodeId, f32)> = None;

        for (&node_id, rect) in &self.node_rects {
            if rect.contains(pos) {
                let area = rect.width() * rect.height();
                match &best {
                    Some((_, best_area)) if area < *best_area => {
                        best = Some((node_id, area));
                    }
                    None => {
                        best = Some((node_id, area));
                    }
                    _ => {}
                }
            }
        }

        best.map(|(id, _)| id)
    }

    /// Test if a position hits any member, returning the (node_id, member_id).
    pub fn hit_test_member(&self, pos: Vec2) -> Option<(NodeId, NodeId)> {
        for (&node_id, members) in &self.member_rects {
            for (&member_id, rect) in members {
                if rect.contains(pos) {
                    return Some((node_id, member_id));
                }
            }
        }
        None
    }

    /// Test if a position hits any edge within the given tolerance.
    ///
    /// Uses uniform sampling along bezier curves to find the closest edge.
    /// Returns the `EdgeId` of the closest edge within tolerance, or `None`.
    ///
    /// # Arguments
    /// * `pos` - The position to test.
    /// * `tolerance` - Maximum distance (in pixels) from the curve to count as a hit.
    ///
    /// **Validates: Requirements 11.2**
    pub fn hit_test_edge(&self, pos: Vec2, tolerance: f32) -> Option<EdgeId> {
        let mut best_id = None;
        let mut best_dist = tolerance;

        for (&edge_id, curve) in &self.edge_curves {
            let dist = curve.point_distance(pos, self.bezier_samples);
            if dist < best_dist {
                best_dist = dist;
                best_id = Some(edge_id);
            }
        }

        best_id
    }

    /// Test if a position hits any edge bundle within the given tolerance.
    fn hit_test_edge_bundle(&self, pos: Vec2, tolerance: f32) -> Option<&EdgeBundleRegion> {
        let mut best: Option<(usize, f32)> = None;

        for (i, region) in self.edge_bundle_regions.iter().enumerate() {
            let dist = region.curve.point_distance(pos, self.bezier_samples);
            if dist < tolerance {
                match &best {
                    Some((_, best_dist)) if dist < *best_dist => {
                        best = Some((i, dist));
                    }
                    None => {
                        best = Some((i, dist));
                    }
                    _ => {}
                }
            }
        }

        best.map(|(i, _)| &self.edge_bundle_regions[i])
    }

    /// Convert an `EdgeRoute` into a `CubicBezier` curve.
    ///
    /// If the edge route has exactly 2 control points, they are used as the
    /// inner control points of the cubic bezier. Otherwise, a straight line
    /// is used (control points at 1/3 and 2/3 of the line).
    fn edge_route_to_curve(edge: &EdgeRoute) -> CubicBezier {
        let start = edge.source.position;
        let end = edge.target.position;

        if edge.control_points.len() == 2 {
            CubicBezier {
                start,
                control1: edge.control_points[0],
                control2: edge.control_points[1],
                end,
            }
        } else {
            // Straight line fallback: control points at 1/3 and 2/3
            let dx = end.x - start.x;
            let dy = end.y - start.y;
            CubicBezier {
                start,
                control1: Vec2::new(start.x + dx / 3.0, start.y + dy / 3.0),
                control2: Vec2::new(start.x + 2.0 * dx / 3.0, start.y + 2.0 * dy / 3.0),
                end,
            }
        }
    }

    // -- Accessors for testing --

    /// Get the node rects (for testing).
    pub fn node_rects(&self) -> &HashMap<NodeId, Rect> {
        &self.node_rects
    }

    /// Get the member rects (for testing).
    pub fn member_rects(&self) -> &HashMap<NodeId, HashMap<NodeId, Rect>> {
        &self.member_rects
    }

    /// Get the edge curves (for testing).
    pub fn edge_curves(&self) -> &HashMap<EdgeId, CubicBezier> {
        &self.edge_curves
    }

    /// Get the edge bundle regions (for testing).
    pub fn edge_bundle_regions(&self) -> &[EdgeBundleRegion] {
        &self.edge_bundle_regions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uml_types::{AnchorSide, EdgeAnchor, UmlNode};
    use codestory_core::{EdgeKind, NodeKind};

    fn make_rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_pos_size(Vec2::new(x, y), Vec2::new(w, h))
    }

    fn make_edge_route(
        id: i64,
        src_node: i64,
        src_pos: Vec2,
        tgt_node: i64,
        tgt_pos: Vec2,
        control_points: Vec<Vec2>,
    ) -> EdgeRoute {
        EdgeRoute::with_control_points(
            EdgeId(id),
            EdgeAnchor::new(NodeId(src_node), src_pos, AnchorSide::Right),
            EdgeAnchor::new(NodeId(tgt_node), tgt_pos, AnchorSide::Left),
            EdgeKind::CALL,
            control_points,
        )
    }

    #[test]
    fn test_hit_test_node() {
        let mut tester = HitTester::new();
        let mut nodes = HashMap::new();

        let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "ClassA".to_string());
        node.computed_rect = make_rect(100.0, 100.0, 200.0, 150.0);
        nodes.insert(NodeId(1), node);

        tester.update(&nodes, &[]);

        // Inside node
        assert_eq!(
            tester.hit_test(Vec2::new(150.0, 150.0)),
            HitResult::Node(NodeId(1))
        );

        // Outside node
        assert_eq!(tester.hit_test(Vec2::new(50.0, 50.0)), HitResult::None);
    }

    #[test]
    fn test_hit_test_member() {
        let mut tester = HitTester::new();
        let mut nodes = HashMap::new();

        let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "ClassA".to_string());
        node.computed_rect = make_rect(100.0, 100.0, 200.0, 150.0);

        // Member rect inside the node
        let member_rect = make_rect(110.0, 140.0, 180.0, 20.0);
        node.member_rects.insert(NodeId(10), member_rect);
        nodes.insert(NodeId(1), node);

        tester.update(&nodes, &[]);

        // Hit the member
        let result = tester.hit_test(Vec2::new(150.0, 145.0));
        assert_eq!(
            result,
            HitResult::Member {
                node_id: NodeId(1),
                member_id: NodeId(10),
            }
        );

        // Hit the node but not the member
        let result = tester.hit_test(Vec2::new(150.0, 110.0));
        assert_eq!(result, HitResult::Node(NodeId(1)));
    }

    #[test]
    fn test_hit_test_edge() {
        let mut tester = HitTester::new();

        // Create a straight horizontal edge from (0,0) to (100,0)
        let edge = make_edge_route(
            1,
            1,
            Vec2::new(0.0, 0.0),
            2,
            Vec2::new(100.0, 0.0),
            vec![Vec2::new(33.0, 0.0), Vec2::new(66.0, 0.0)],
        );

        tester.update(&HashMap::new(), &[edge]);

        // Near the edge (within tolerance)
        let result = tester.hit_test_edge(Vec2::new(50.0, 3.0), 8.0);
        assert_eq!(result, Some(EdgeId(1)));

        // Far from the edge
        let result = tester.hit_test_edge(Vec2::new(50.0, 50.0), 8.0);
        assert_eq!(result, None);
    }

    #[test]
    fn test_hit_test_edge_with_tolerance() {
        let mut tester = HitTester::new();

        let edge = make_edge_route(
            1,
            1,
            Vec2::new(0.0, 0.0),
            2,
            Vec2::new(100.0, 0.0),
            vec![Vec2::new(33.0, 0.0), Vec2::new(66.0, 0.0)],
        );

        tester.update(&HashMap::new(), &[edge]);

        // With small tolerance -- should miss
        let result = tester.hit_test_edge(Vec2::new(50.0, 5.0), 2.0);
        assert_eq!(result, None);

        // With larger tolerance -- should hit
        let result = tester.hit_test_edge(Vec2::new(50.0, 5.0), 8.0);
        assert_eq!(result, Some(EdgeId(1)));
    }

    #[test]
    fn test_hit_test_edge_bundle() {
        let mut tester = HitTester::new();
        tester.update(&HashMap::new(), &[]);

        let curve = CubicBezier {
            start: Vec2::new(0.0, 0.0),
            control1: Vec2::new(33.0, 0.0),
            control2: Vec2::new(66.0, 0.0),
            end: Vec2::new(100.0, 0.0),
        };

        tester.add_edge_bundle(0, vec![EdgeId(1), EdgeId(2), EdgeId(3)], curve);

        // Near the bundle
        let result = tester.hit_test(Vec2::new(50.0, 3.0));
        assert!(matches!(result, HitResult::EdgeBundle { bundle_id: 0, .. }));
    }

    #[test]
    fn test_priority_member_over_node() {
        let mut tester = HitTester::new();
        let mut nodes = HashMap::new();

        let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "ClassA".to_string());
        node.computed_rect = make_rect(0.0, 0.0, 200.0, 200.0);
        node.member_rects
            .insert(NodeId(10), make_rect(10.0, 50.0, 180.0, 20.0));
        nodes.insert(NodeId(1), node);

        tester.update(&nodes, &[]);

        // Point inside both node and member -- member should win
        let result = tester.hit_test(Vec2::new(100.0, 55.0));
        assert_eq!(
            result,
            HitResult::Member {
                node_id: NodeId(1),
                member_id: NodeId(10),
            }
        );
    }

    #[test]
    fn test_priority_node_over_edge() {
        let mut tester = HitTester::new();
        let mut nodes = HashMap::new();

        let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "ClassA".to_string());
        node.computed_rect = make_rect(40.0, -10.0, 20.0, 20.0);
        nodes.insert(NodeId(1), node);

        // Edge passes through the node area
        let edge = make_edge_route(
            1,
            2,
            Vec2::new(0.0, 0.0),
            3,
            Vec2::new(100.0, 0.0),
            vec![Vec2::new(33.0, 0.0), Vec2::new(66.0, 0.0)],
        );

        tester.update(&nodes, &[edge]);

        // Point inside the node (which also overlaps the edge)
        let result = tester.hit_test(Vec2::new(50.0, 0.0));
        assert_eq!(result, HitResult::Node(NodeId(1)));
    }

    #[test]
    fn test_smallest_node_wins_on_overlap() {
        let mut tester = HitTester::new();
        let mut nodes = HashMap::new();

        // Large node
        let mut large_node = UmlNode::new(NodeId(1), NodeKind::CLASS, "LargeClass".to_string());
        large_node.computed_rect = make_rect(0.0, 0.0, 300.0, 300.0);
        nodes.insert(NodeId(1), large_node);

        // Small node inside the large one
        let mut small_node = UmlNode::new(NodeId(2), NodeKind::CLASS, "SmallClass".to_string());
        small_node.computed_rect = make_rect(100.0, 100.0, 50.0, 50.0);
        nodes.insert(NodeId(2), small_node);

        tester.update(&nodes, &[]);

        // Point inside both -- smaller node wins
        let result = tester.hit_test(Vec2::new(120.0, 120.0));
        assert_eq!(result, HitResult::Node(NodeId(2)));

        // Point inside only the large node
        let result = tester.hit_test(Vec2::new(10.0, 10.0));
        assert_eq!(result, HitResult::Node(NodeId(1)));
    }

    #[test]
    fn test_update_clears_previous_data() {
        let mut tester = HitTester::new();
        let mut nodes = HashMap::new();

        let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "ClassA".to_string());
        node.computed_rect = make_rect(0.0, 0.0, 100.0, 100.0);
        nodes.insert(NodeId(1), node);

        tester.update(&nodes, &[]);
        assert!(tester.node_rects().contains_key(&NodeId(1)));

        // Update with empty data
        tester.update(&HashMap::new(), &[]);
        assert!(tester.node_rects().is_empty());
    }

    #[test]
    fn test_update_from_rects() {
        let mut tester = HitTester::new();

        let mut node_rects = HashMap::new();
        node_rects.insert(NodeId(1), make_rect(0.0, 0.0, 100.0, 100.0));

        let mut member_rects = HashMap::new();
        let mut members = HashMap::new();
        members.insert(NodeId(10), make_rect(10.0, 30.0, 80.0, 20.0));
        member_rects.insert(NodeId(1), members);

        tester.update_from_rects(node_rects, member_rects, &[]);

        let result = tester.hit_test(Vec2::new(50.0, 35.0));
        assert_eq!(
            result,
            HitResult::Member {
                node_id: NodeId(1),
                member_id: NodeId(10),
            }
        );
    }

    #[test]
    fn test_edge_route_to_curve_with_control_points() {
        let edge = make_edge_route(
            1,
            1,
            Vec2::new(0.0, 0.0),
            2,
            Vec2::new(100.0, 50.0),
            vec![Vec2::new(30.0, 10.0), Vec2::new(70.0, 40.0)],
        );

        let curve = HitTester::edge_route_to_curve(&edge);
        assert_eq!(curve.start, Vec2::new(0.0, 0.0));
        assert_eq!(curve.control1, Vec2::new(30.0, 10.0));
        assert_eq!(curve.control2, Vec2::new(70.0, 40.0));
        assert_eq!(curve.end, Vec2::new(100.0, 50.0));
    }

    #[test]
    fn test_edge_route_to_curve_straight_line() {
        let edge = make_edge_route(
            1,
            1,
            Vec2::new(0.0, 0.0),
            2,
            Vec2::new(90.0, 0.0),
            vec![], // No control points
        );

        let curve = HitTester::edge_route_to_curve(&edge);
        assert_eq!(curve.start, Vec2::new(0.0, 0.0));
        assert_eq!(curve.end, Vec2::new(90.0, 0.0));
        // Control points should be at 1/3 and 2/3
        assert_eq!(curve.control1, Vec2::new(30.0, 0.0));
        assert_eq!(curve.control2, Vec2::new(60.0, 0.0));
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use crate::uml_types::UmlNode;
    use codestory_core::NodeKind;
    use proptest::prelude::*;

    /// Strategy for generating a rectangle with known position and size.
    fn rect_strategy() -> impl Strategy<Value = Rect> {
        (0.0f32..500.0, 0.0f32..500.0, 20.0f32..200.0, 20.0f32..200.0)
            .prop_map(|(x, y, w, h)| Rect::from_pos_size(Vec2::new(x, y), Vec2::new(w, h)))
    }

    proptest! {
        /// **Validates: Property 26: Hit Test Correctness**
        ///
        /// For any click position P within a node's bounding box, hit_test(P) SHALL
        /// return that node's NodeId. For any position P within a member's row,
        /// hit_test(P) SHALL return that member's NodeId.
        #[test]
        fn prop_hit_test_node_correctness(
            node_rect in rect_strategy()
        ) {
            // Ensure minimum size for valid range generation
            prop_assume!(node_rect.width() > 1.0 && node_rect.height() > 1.0);

            let mut tester = HitTester::new();
            let mut nodes = HashMap::new();

            let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());
            node.computed_rect = node_rect;
            nodes.insert(NodeId(1), node);

            tester.update(&nodes, &[]);

            // Test a point inside the node
            let center = node_rect.center();
            let result = tester.hit_test(center);

            prop_assert_eq!(
                result,
                HitResult::Node(NodeId(1)),
                "Hit test at center {:?} of node rect {:?} should return Node(1)",
                center,
                node_rect
            );
        }

        /// **Validates: Property 26 (member variant)**
        ///
        /// For any position P within a member's row, hit_test(P) SHALL return
        /// that member's NodeId (as HitResult::Member).
        #[test]
        fn prop_hit_test_member_correctness(
            node_rect in rect_strategy()
        ) {
            // Ensure node is large enough to contain a member
            prop_assume!(node_rect.width() > 20.0 && node_rect.height() > 60.0);

            let mut tester = HitTester::new();
            let mut nodes = HashMap::new();

            let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());
            node.computed_rect = node_rect;

            // Place a member rect inside the node
            let member_rect = Rect::from_pos_size(
                Vec2::new(node_rect.min.x + 5.0, node_rect.min.y + 35.0),
                Vec2::new(node_rect.width() - 10.0, 20.0),
            );

            // Make sure member is within node
            prop_assume!(member_rect.max.y <= node_rect.max.y);

            node.member_rects.insert(NodeId(10), member_rect);
            nodes.insert(NodeId(1), node);

            tester.update(&nodes, &[]);

            // Test a point inside the member rect
            let member_center = member_rect.center();
            let result = tester.hit_test(member_center);

            prop_assert_eq!(
                result,
                HitResult::Member {
                    node_id: NodeId(1),
                    member_id: NodeId(10),
                },
                "Hit test at member center {:?} should return Member {{ node_id: 1, member_id: 10 }}",
                member_center
            );
        }

        /// **Validates: Property 26 (priority)**
        ///
        /// Member hits always take priority over node hits.
        #[test]
        fn prop_hit_test_member_priority_over_node(
            node_rect in rect_strategy()
        ) {
            prop_assume!(node_rect.width() > 30.0 && node_rect.height() > 70.0);

            let mut tester = HitTester::new();
            let mut nodes = HashMap::new();

            let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());
            node.computed_rect = node_rect;

            let member_rect = Rect::from_pos_size(
                Vec2::new(node_rect.min.x + 5.0, node_rect.min.y + 40.0),
                Vec2::new(node_rect.width() - 10.0, 20.0),
            );

            prop_assume!(member_rect.max.y <= node_rect.max.y);

            node.member_rects.insert(NodeId(10), member_rect);
            nodes.insert(NodeId(1), node);

            tester.update(&nodes, &[]);

            // The member center is also inside the node rect.
            // Member should win.
            let test_point = member_rect.center();
            let result = tester.hit_test(test_point);

            prop_assert!(
                matches!(result, HitResult::Member { node_id: NodeId(1), member_id: NodeId(10) }),
                "Member should have priority over node. Got {:?} at {:?}",
                result,
                test_point
            );
        }

        /// **Validates: Property 26 (outside)**
        ///
        /// For any position P outside all node bounding boxes, hit_test(P) SHALL NOT
        /// return a Node or Member result (only Edge, EdgeBundle, or None).
        #[test]
        fn prop_hit_test_outside_returns_none(
            node_rect in rect_strategy(),
            offset_x in 10.0f32..100.0,
            offset_y in 10.0f32..100.0,
            quadrant in 0u8..4
        ) {
            let mut tester = HitTester::new();
            let mut nodes = HashMap::new();

            let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());
            node.computed_rect = node_rect;
            nodes.insert(NodeId(1), node);

            // No edges, so testing should return None for outside points
            tester.update(&nodes, &[]);

            // Generate a point guaranteed to be outside the node rect
            let outside_point = match quadrant {
                0 => Vec2::new(node_rect.min.x - offset_x, node_rect.min.y - offset_y),
                1 => Vec2::new(node_rect.max.x + offset_x, node_rect.min.y - offset_y),
                2 => Vec2::new(node_rect.min.x - offset_x, node_rect.max.y + offset_y),
                _ => Vec2::new(node_rect.max.x + offset_x, node_rect.max.y + offset_y),
            };

            let result = tester.hit_test(outside_point);

            prop_assert_eq!(
                result,
                HitResult::None,
                "Point {:?} outside node rect {:?} should return None",
                outside_point,
                node_rect
            );
        }
    }
}
