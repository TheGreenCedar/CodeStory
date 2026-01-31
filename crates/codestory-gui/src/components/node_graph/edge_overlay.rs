use codestory_core::{EdgeKind, NodeId};
use codestory_graph::edge_router::{CubicBezier, EdgeRouter, BUNDLE_THRESHOLD};
use codestory_graph::style::{self, get_edge_kind_label};
use codestory_graph::DummyEdge;
use eframe::egui::{self, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use std::collections::{BTreeMap, HashMap, HashSet};
use crate::components::node_graph::style_resolver::StyleResolver;

fn to_graph_rect(r: Rect) -> codestory_graph::uml_types::Rect {
    codestory_graph::uml_types::Rect {
        min: codestory_graph::Vec2 {
            x: r.min.x,
            y: r.min.y,
        },
        max: codestory_graph::Vec2 {
            x: r.max.x,
            y: r.max.y,
        },
    }
}

/// Key for a bundle: ordered pair of (source, target) node IDs.
type BundleKey = (NodeId, NodeId);

/// Information about a rendered edge, used for hit testing and interaction.
///
/// **Validates: Requirements 8.1, 8.2, 8.3, 11.2**
#[derive(Debug, Clone)]
pub struct EdgeHitInfo {
    /// The edge kind (CALL, INHERITANCE, MEMBER, etc.)
    pub kind: EdgeKind,
    /// Source node ID
    pub source: NodeId,
    /// Target node ID
    pub target: NodeId,
    /// Source node label (for tooltip)
    pub source_label: String,
    /// Target node label (for tooltip)
    pub target_label: String,
    /// The bezier curve in screen space (for hit testing)
    pub screen_curve: CubicBezier,
    /// Whether this edge is part of a bundle
    #[allow(dead_code)]
    pub is_bundled: bool,
}

/// Result of an edge selection, returned to the viewer for side panel display.
///
/// **Validates: Requirement 8.3**
#[derive(Debug, Clone)]
pub struct SelectedEdgeInfo {
    /// The edge kind
    pub kind: EdgeKind,
    /// Source node ID
    pub source: NodeId,
    /// Target node ID
    pub target: NodeId,
    /// Source node label
    #[allow(dead_code)]
    pub source_label: String,
    /// Target node label
    #[allow(dead_code)]
    pub target_label: String,
}

/// Renders edges as a custom overlay on top (or behind) the graph nodes.
///
/// This component is responsible for drawing bezier curves, arrowheads, and
/// handling edge bundling visualization. It bypasses the default wire rendering
/// rendering to provide Sourcetrail-like visuals.
///
/// Phase 8 additions:
/// - Count badge on bundled edges (Req 14.2)
/// - Tooltip listing all bundled relationships on hover (Req 14.3)
/// - Click to expand bundle showing individual edges temporarily (Req 14.4)
///
/// Phase 9 additions:
/// - Hit testing for individual edges with tolerance (Req 8.1, 11.2)
/// - Edge hover highlighting with connected node highlighting (Req 8.1, 4.3, Property 13)
/// - Edge hover tooltip with type and symbol names (Req 8.2)
/// - Edge click selection for side panel (Req 8.3)
pub struct EdgeOverlay {
    pub router: EdgeRouter,

    /// Set of bundle keys that are currently expanded (showing individual edges).
    /// When a bundled edge group is clicked, its key is toggled in this set.
    ///
    /// **Validates: Requirements 14.4, Property 32: Edge Bundle Expansion**
    expanded_bundles: HashSet<BundleKey>,

    /// Screen-space midpoints of rendered bundles for hit testing (hover/click).
    /// Populated each frame during render. Maps bundle key to the screen-space
    /// midpoint and edge count.
    bundle_midpoints:
        HashMap<BundleKey, (Pos2, usize, Vec<(codestory_core::EdgeKind, String, String)>)>,

    /// The bundle key currently being hovered (if any).
    hovered_bundle: Option<BundleKey>,

    /// Per-frame list of rendered edge hit regions for individual edge hit testing.
    /// Populated during render, cleared at start of each frame.
    ///
    /// **Validates: Requirements 8.1, 11.2**
    pub(crate) edge_hit_infos: Vec<EdgeHitInfo>,

    /// The index into `edge_hit_infos` of the currently hovered edge (if any).
    ///
    /// **Validates: Requirement 8.1, Property 13**
    hovered_edge_index: Option<usize>,

    /// The currently selected edge info (persists across frames until deselected).
    ///
    /// **Validates: Requirement 8.3**
    selected_edge: Option<SelectedEdgeInfo>,

    /// Set of node IDs that should be highlighted because they are connected
    /// to a hovered or selected edge. Updated each frame.
    ///
    /// **Validates: Requirement 4.3, Property 13**
    pub highlighted_nodes: HashSet<NodeId>,

    /// Tolerance in screen pixels for edge hit testing.
    /// Points within this distance of a bezier curve count as "hovering" the edge.
    ///
    /// **Validates: Requirement 11.2**
    pub hit_tolerance: f32,

    /// Node name lookup populated each frame from the graph data.
    /// Maps NodeId to the display label for tooltips.
    node_labels: HashMap<NodeId, String>,
}

impl Default for EdgeOverlay {
    fn default() -> Self {
        Self {
            router: EdgeRouter::default(),
            expanded_bundles: HashSet::new(),
            bundle_midpoints: HashMap::new(),
            hovered_bundle: None,
            edge_hit_infos: Vec::new(),
            hovered_edge_index: None,
            selected_edge: None,
            highlighted_nodes: HashSet::new(),
            hit_tolerance: 6.0,
            node_labels: HashMap::new(),
        }
    }
}

impl EdgeOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle the expansion state of a bundle. If expanded, it collapses; if collapsed, it expands.
    ///
    /// **Validates: Requirements 14.4, Property 32**
    pub fn toggle_bundle_expanded(&mut self, key: BundleKey) {
        if self.expanded_bundles.contains(&key) {
            self.expanded_bundles.remove(&key);
        } else {
            self.expanded_bundles.insert(key);
        }
    }

    /// Check if a bundle is currently expanded.
    pub fn is_bundle_expanded(&self, key: &BundleKey) -> bool {
        self.expanded_bundles.contains(key)
    }

    /// Get the currently selected edge info, if any.
    ///
    /// **Validates: Requirement 8.3**
    #[allow(dead_code)]
    pub fn get_selected_edge(&self) -> Option<&SelectedEdgeInfo> {
        self.selected_edge.as_ref()
    }

    /// Clear the currently selected edge.
    #[allow(dead_code)]
    pub fn clear_selection(&mut self) {
        self.selected_edge = None;
    }

    pub fn hovered_edge_info(&self) -> Option<&EdgeHitInfo> {
        self.hovered_edge_index
            .and_then(|idx| self.edge_hit_infos.get(idx))
    }

    pub fn clear_frame_state(&mut self) {
        self.bundle_midpoints.clear();
        self.hovered_bundle = None;
        self.edge_hit_infos.clear();
        self.highlighted_nodes.clear();
        self.hovered_edge_index = None;
    }

    /// Hit test: find the closest edge to the given screen-space point.
    ///
    /// Returns the index into `edge_hit_infos` if any edge is within `hit_tolerance`.
    /// Uses uniform sampling along each bezier curve.
    ///
    /// **Validates: Requirements 8.1, 11.2**
    pub fn hit_test(&self, screen_pos: Pos2) -> Option<usize> {
        let test_point = codestory_graph::Vec2 {
            x: screen_pos.x,
            y: screen_pos.y,
        };

        let mut best_index = None;
        let mut best_dist = self.hit_tolerance;

        for (i, info) in self.edge_hit_infos.iter().enumerate() {
            let dist = info.screen_curve.point_distance(test_point, 64);
            if dist < best_dist {
                best_dist = dist;
                best_index = Some(i);
            }
        }

        best_index
    }

    /// Set node labels for tooltip display.
    /// Call this each frame with the current node label mapping.
    pub fn set_node_labels(&mut self, labels: HashMap<NodeId, String>) {
        self.node_labels = labels;
    }

    /// Get the label for a node ID, falling back to a numeric representation.
    fn get_node_label(&self, id: NodeId) -> String {
        self.node_labels
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("Node({})", id.0))
    }

    /// Render all edges.
    ///
    /// # Arguments
    /// * `ui` - The ui to paint into.
    /// * `_bg_painter` - Background painter (unused, kept for API compat).
    /// * `painter` - The painter to use (allows specifying layer).
    /// * `edges` - List of edges to draw.
    /// * `node_rects` - Map of NodeId to their *graph space* bounding rectangles.
    /// * `visible_rect` - The visible area of the viewport in screen coordinates (for culling).
    /// * `transform` - Graph-space to screen-space transform.
    /// * `node_inner_margin` - Inner margin of node frames (for rect expansion).
    /// * `style_resolver` - The style resolver for determining edge colors.
    pub fn render(
        &mut self,
        ui: &egui::Ui,
        _bg_painter: &Painter,
        painter: &Painter,
        edges: &[DummyEdge],
        node_rects: &HashMap<NodeId, Rect>,
        visible_rect: Rect,
        transform: egui::emath::TSTransform,
        node_inner_margin: egui::Margin,
        style_resolver: &StyleResolver,
    ) {
        // Clear per-frame data
        self.bundle_midpoints.clear();
        self.hovered_bundle = None;
        self.edge_hit_infos.clear();
        self.highlighted_nodes.clear();

        // Group edges by (source, target) for bundling
        // Use BTreeMap for stable ordering to prevent Z-fighting flickering
        let mut bundles: BTreeMap<BundleKey, Vec<&DummyEdge>> = BTreeMap::new();
        for edge in edges {
            bundles
                .entry((edge.source, edge.target))
                .or_default()
                .push(edge);
        }

        let mouse_pos = ui.ctx().pointer_hover_pos().filter(|pos| visible_rect.contains(*pos));

        for ((source, target), bundle) in &bundles {
            if let (Some(source_rect_graph), Some(target_rect_graph)) =
                (node_rects.get(source), node_rects.get(target))
            {
                // Culling: check if either node is visible
                let source_rect_screen = transform * *source_rect_graph;
                let target_rect_screen = transform * *target_rect_graph;

                let bundle_bounds = source_rect_screen.union(target_rect_screen);
                if !visible_rect.intersects(bundle_bounds.expand(100.0)) {
                    continue;
                }

                let scale = transform.scaling;

                // Expand the graph-space rect by the node's visual margin
                let mut source_rect = *source_rect_graph;
                source_rect.min.x -= node_inner_margin.left as f32 / scale;
                source_rect.max.x += node_inner_margin.right as f32 / scale;
                source_rect.min.y -= node_inner_margin.top as f32 / scale;
                source_rect.max.y += node_inner_margin.bottom as f32 / scale;

                let mut target_rect = *target_rect_graph;
                target_rect.min.x -= node_inner_margin.left as f32 / scale;
                target_rect.max.x += node_inner_margin.right as f32 / scale;
                target_rect.min.y -= node_inner_margin.top as f32 / scale;
                target_rect.max.y += node_inner_margin.bottom as f32 / scale;

                let key = (*source, *target);
                let is_expanded = self.is_bundle_expanded(&key);

                self.render_bundle_group(
                    painter,
                    painter,
                    bundle,
                    source_rect,
                    target_rect,
                    transform,
                    key,
                    is_expanded,
                    style_resolver,
                );
            }
        }

        // --- Phase 9: Edge hover and selection interaction ---

        // First, handle edge hit testing for individual edges (Task 9.1, 9.2)
        self.hovered_edge_index = None;
        if let Some(mouse) = mouse_pos {
            // Check bundle badge hover first (bundles take priority)
            let badge_radius = 10.0;
            let mut on_bundle = false;
            for (key, (midpoint, count, relationships)) in &self.bundle_midpoints {
                let dist = midpoint.distance(mouse);
                if dist <= badge_radius + 4.0 {
                    self.hovered_bundle = Some(*key);
                    on_bundle = true;

                    // Show tooltip listing all bundled relationships (Req 14.3)
                    #[allow(deprecated)]
                    egui::show_tooltip_at_pointer(
                        ui.ctx(),
                        ui.layer_id(),
                        egui::Id::new("edge_bundle_tooltip"),
                        |ui| {
                            ui.label(
                                egui::RichText::new(format!("{} bundled edges", count)).strong(),
                            );
                            ui.separator();
                            for (kind, source_label, target_label) in relationships {
                                let kind_label = get_edge_kind_label(*kind);
                                ui.label(format!(
                                    "{} {} {}",
                                    source_label, kind_label, target_label
                                ));
                            }
                        },
                    );
                    break;
                }
            }

            // If not hovering a bundle badge, check individual edges (Task 9.1)
            if !on_bundle {
                if let Some(idx) = self.hit_test(mouse) {
                    self.hovered_edge_index = Some(idx);

                    // Highlight connected nodes (Task 9.2, Req 4.3, Property 13)
                    let info = &self.edge_hit_infos[idx];
                    self.highlighted_nodes.insert(info.source);
                    self.highlighted_nodes.insert(info.target);

                    // Show tooltip with edge type and connected symbol names (Task 9.3, Req 8.2)
                    let kind_label = get_edge_kind_label(info.kind);
                    let source_label = info.source_label.clone();
                    let target_label = info.target_label.clone();
                    #[allow(deprecated)]
                    egui::show_tooltip_at_pointer(
                        ui.ctx(),
                        ui.layer_id(),
                        egui::Id::new("edge_hover_tooltip"),
                        |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} -> {}",
                                    source_label, target_label
                                ))
                                .strong(),
                            );
                            ui.label(format!("Type: {}", kind_label));
                        },
                    );
                }
            }
        }

        // Draw hover highlight overlay on hovered edge (Task 9.2)
        if let Some(idx) = self.hovered_edge_index {
            let info = &self.edge_hit_infos[idx];
            let base_color = style_resolver.resolve_edge_color(info.kind);
            let highlight_color = Color32::from_rgba_unmultiplied(
                base_color.r().saturating_add(60),
                base_color.g().saturating_add(60),
                base_color.b().saturating_add(60),
                255,
            );
            let highlight_stroke = Stroke::new(4.0, highlight_color);

            let curve = &info.screen_curve;
            let start = Pos2::new(curve.start.x, curve.start.y);
            let cp1 = Pos2::new(curve.control1.x, curve.control1.y);
            let cp2 = Pos2::new(curve.control2.x, curve.control2.y);
            let end = Pos2::new(curve.end.x, curve.end.y);

            use egui::epaint::CubicBezierShape;
            let shape = CubicBezierShape::from_points_stroke(
                [start, cp1, cp2, end],
                false,
                Color32::TRANSPARENT,
                highlight_stroke,
            );
            painter.add(shape);
        }

        // Draw selection highlight on selected edge
        if let Some(sel) = &self.selected_edge {
            // Find matching edge in current hit infos
            for info in &self.edge_hit_infos {
                if info.source == sel.source && info.target == sel.target && info.kind == sel.kind {
                    let sel_color = Color32::from_rgb(255, 200, 100); // Focus border color
                    let sel_stroke = Stroke::new(3.0, sel_color);

                    let curve = &info.screen_curve;
                    let start = Pos2::new(curve.start.x, curve.start.y);
                    let cp1 = Pos2::new(curve.control1.x, curve.control1.y);
                    let cp2 = Pos2::new(curve.control2.x, curve.control2.y);
                    let end = Pos2::new(curve.end.x, curve.end.y);

                    use egui::epaint::CubicBezierShape;
                    let shape = CubicBezierShape::from_points_stroke(
                        [start, cp1, cp2, end],
                        false,
                        Color32::TRANSPARENT,
                        sel_stroke,
                    );
                    painter.add(shape);

                    // Also highlight connected nodes for selected edge
                    self.highlighted_nodes.insert(sel.source);
                    self.highlighted_nodes.insert(sel.target);
                    break;
                }
            }
        }

        // Handle click interactions (Task 9.4, Req 8.3)
        if ui.input(|i| i.pointer.primary_clicked()) {
            if let Some(mouse) = mouse_pos {
                let badge_radius = 10.0;
                let mut clicked_bundle = false;

                // Check bundle badge click first
                for (key, (midpoint, _count, _)) in &self.bundle_midpoints {
                    let dist = midpoint.distance(mouse);
                    if dist <= badge_radius + 4.0 {
                        let key_copy = *key;
                        self.toggle_bundle_expanded(key_copy);
                        clicked_bundle = true;
                        break;
                    }
                }

                // If not clicking a bundle badge, check individual edge click (Task 9.4)
                if !clicked_bundle {
                    if let Some(idx) = self.hit_test(mouse) {
                        let info = &self.edge_hit_infos[idx];
                        self.selected_edge = Some(SelectedEdgeInfo {
                            kind: info.kind,
                            source: info.source,
                            target: info.target,
                            source_label: info.source_label.clone(),
                            target_label: info.target_label.clone(),
                        });
                    }
                    // Note: clicking empty space does NOT deselect (user can call clear_selection)
                }
            }
        }
    }

    fn render_bundle_group(
        &mut self,
        bg_painter: &Painter,
        fg_painter: &Painter,
        bundle: &[&DummyEdge],
        source_rect: Rect,
        target_rect: Rect,
        transform: egui::emath::TSTransform,
        key: BundleKey,
        is_expanded: bool,
        style_resolver: &StyleResolver,
    ) {
        let count = bundle.len();
        if count == 0 {
            return;
        }

        let is_bundled = count >= BUNDLE_THRESHOLD;

        let to_screen = |v: codestory_graph::Vec2| -> Pos2 { transform * Pos2::new(v.x, v.y) };

        if is_bundled && !is_expanded {
            // Determine dominant edge kind by priority
            let dominant_kind = bundle
                .iter()
                .map(|e| e.kind)
                .max_by_key(|&kind| match kind {
                    codestory_core::EdgeKind::INHERITANCE => 3,
                    codestory_core::EdgeKind::CALL => 2,
                    codestory_core::EdgeKind::MEMBER => 1,
                    _ => 0,
                })
                .unwrap_or(codestory_core::EdgeKind::MEMBER);

            // Route in graph space
            let curve = self
                .router
                .route_edge(to_graph_rect(source_rect), to_graph_rect(target_rect));

            // Use proper color and width from style system (logarithmic thickness)
            let edge_color = style_resolver.resolve_edge_color(dominant_kind);
            let bundle_color = style_resolver.resolve_bundled_edge_color();
            let color = mix_color(edge_color, bundle_color, 0.35);
            let width = style::edge_width(dominant_kind, count) * style_resolver.edge_width_scale();
            let stroke = Stroke::new(width, color);

            // Project to screen space
            let start = to_screen(curve.start);
            let cp1 = to_screen(curve.control1);
            let cp2 = to_screen(curve.control2);
            let end = to_screen(curve.end);

            use egui::epaint::CubicBezierShape;
            let shape = CubicBezierShape::from_points_stroke(
                [start, cp1, cp2, end],
                false,
                Color32::TRANSPARENT,
                stroke,
            );
            bg_painter.add(shape);

            // Draw arrowhead for bundled edge
            let t_start = to_screen(curve.control2);
            let tangent = codestory_graph::Vec2 {
                x: end.x - t_start.x,
                y: end.y - t_start.y,
            };
            self.draw_scaled_arrowhead(fg_painter, end, tangent, color, 7.0, 3.0);

            // --- Count badge (Req 14.2, Property 30) ---
            // Place badge at the midpoint of the curve (t=0.5), but
            // nudge it away if it overlaps the node bodies.
            let midpoint = to_screen(curve.sample(0.5));
            let source_rect_screen = transform * source_rect;
            let target_rect_screen = transform * target_rect;
            let badge_center = self.resolve_badge_center(
                fg_painter,
                midpoint,
                count,
                start,
                cp1,
                cp2,
                end,
                source_rect_screen,
                target_rect_screen,
            );
            self.draw_count_badge(fg_painter, badge_center, count, bundle_color);

            // Store midpoint for hover/click detection
            let relationships: Vec<(codestory_core::EdgeKind, String, String)> = bundle
                .iter()
                .map(|e| {
                    (
                        e.kind,
                        self.get_node_label(e.source),
                        self.get_node_label(e.target),
                    )
                })
                .collect();
            self.bundle_midpoints
                .insert(key, (badge_center, count, relationships));

            // Store hit info for the bundled edge (as a single hit target)
            let screen_curve = CubicBezier {
                start: codestory_graph::Vec2::new(start.x, start.y),
                control1: codestory_graph::Vec2::new(cp1.x, cp1.y),
                control2: codestory_graph::Vec2::new(cp2.x, cp2.y),
                end: codestory_graph::Vec2::new(end.x, end.y),
            };
            self.edge_hit_infos.push(EdgeHitInfo {
                kind: dominant_kind,
                source: key.0,
                target: key.1,
                source_label: self.get_node_label(key.0),
                target_label: self.get_node_label(key.1),
                screen_curve,
                is_bundled: true,
            });
        } else {
            // Draw individual edges (either below threshold, or expanded bundle)
            for edge in bundle {
                let curve = self
                    .router
                    .route_edge(to_graph_rect(source_rect), to_graph_rect(target_rect));

                let color = style_resolver.resolve_edge_color(edge.kind);
                let width =
                    style::edge_width(edge.kind, 1) * style_resolver.edge_width_scale();
                let stroke = Stroke::new(width, color);

                let start = to_screen(curve.start);
                let cp1 = to_screen(curve.control1);
                let cp2 = to_screen(curve.control2);
                let end = to_screen(curve.end);

                use egui::epaint::CubicBezierShape;
                let shape = CubicBezierShape::from_points_stroke(
                    [start, cp1, cp2, end],
                    false,
                    Color32::TRANSPARENT,
                    stroke,
                );
                bg_painter.add(shape);

                // Draw arrowhead
                let t_start = to_screen(curve.control2);
                let tangent = codestory_graph::Vec2 {
                    x: end.x - t_start.x,
                    y: end.y - t_start.y,
                };
                self.draw_scaled_arrowhead(fg_painter, end, tangent, color, 7.0, 3.0);

                // Store hit info for this individual edge (Task 9.1)
                let screen_curve = CubicBezier {
                    start: codestory_graph::Vec2::new(start.x, start.y),
                    control1: codestory_graph::Vec2::new(cp1.x, cp1.y),
                    control2: codestory_graph::Vec2::new(cp2.x, cp2.y),
                    end: codestory_graph::Vec2::new(end.x, end.y),
                };
                self.edge_hit_infos.push(EdgeHitInfo {
                    kind: edge.kind,
                    source: edge.source,
                    target: edge.target,
                    source_label: self.get_node_label(edge.source),
                    target_label: self.get_node_label(edge.target),
                    screen_curve,
                    is_bundled: false,
                });
            }

            // If this was an expanded bundle, show a small "collapse" indicator at the midpoint
            if is_bundled && is_expanded {
                let curve = self
                    .router
                    .route_edge(to_graph_rect(source_rect), to_graph_rect(target_rect));
                let midpoint = to_screen(curve.sample(0.5));
                let start = to_screen(curve.start);
                let cp1 = to_screen(curve.control1);
                let cp2 = to_screen(curve.control2);
                let end = to_screen(curve.end);

                // Draw a smaller badge with a collapse hint
                let dominant_kind = bundle
                    .iter()
                    .map(|e| e.kind)
                    .max_by_key(|&kind| match kind {
                        codestory_core::EdgeKind::INHERITANCE => 3,
                        codestory_core::EdgeKind::CALL => 2,
                        codestory_core::EdgeKind::MEMBER => 1,
                        _ => 0,
                    })
                    .unwrap_or(codestory_core::EdgeKind::MEMBER);
                let edge_color = style_resolver.resolve_edge_color(dominant_kind);
                let bundle_color = style_resolver.resolve_bundled_edge_color();
                let color = mix_color(edge_color, bundle_color, 0.35);

                let source_rect_screen = transform * source_rect;
                let target_rect_screen = transform * target_rect;
                let badge_center = self.resolve_badge_center(
                    fg_painter,
                    midpoint,
                    count,
                    start,
                    cp1,
                    cp2,
                    end,
                    source_rect_screen,
                    target_rect_screen,
                );
                self.draw_count_badge(fg_painter, badge_center, count, color);

                // Store midpoint for interaction
                let relationships: Vec<(codestory_core::EdgeKind, String, String)> = bundle
                    .iter()
                    .map(|e| {
                        (
                            e.kind,
                            self.get_node_label(e.source),
                            self.get_node_label(e.target),
                        )
                    })
                    .collect();
                self.bundle_midpoints
                    .insert(key, (badge_center, count, relationships));
            }
        }
    }

    fn resolve_badge_center(
        &self,
        painter: &Painter,
        center: Pos2,
        count: usize,
        start: Pos2,
        cp1: Pos2,
        cp2: Pos2,
        end: Pos2,
        source_rect: Rect,
        target_rect: Rect,
    ) -> Pos2 {
        let badge_rect = self.compute_badge_rect(painter, center, count);
        let union_rect = source_rect.union(target_rect);
        if !badge_rect.intersects(source_rect) && !badge_rect.intersects(target_rect) {
            return center;
        }

        let t = 0.5;
        let mt = 1.0 - t;

        let tangent = (cp1 - start) * (3.0 * mt * mt)
            + (cp2 - cp1) * (6.0 * mt * t)
            + (end - cp2) * (3.0 * t * t);

        let mut normal = egui::vec2(-tangent.y, tangent.x);
        let normal_len = normal.length();
        if normal_len > 0.001 {
            normal /= normal_len;
        } else {
            normal = egui::vec2(0.0, -1.0);
        }

        let base_offset = badge_rect.size().max_elem() * 0.6 + 6.0;
        for step in 1..=4 {
            let offset = base_offset * step as f32;
            for dir in [1.0, -1.0] {
                let candidate = center + normal * (offset * dir);
                let candidate_rect = self.compute_badge_rect(painter, candidate, count);
                if !candidate_rect.intersects(source_rect)
                    && !candidate_rect.intersects(target_rect)
                {
                    return candidate;
                }
            }
        }

        // Fallback: place the badge just outside the union of both nodes.
        Pos2::new(
            union_rect.max.x + badge_rect.width() / 2.0 + 6.0,
            union_rect.min.y - badge_rect.height() / 2.0 - 6.0,
        )
    }

    fn compute_badge_rect(&self, painter: &Painter, center: Pos2, count: usize) -> Rect {
        let text = format!("{}", count);
        let font_id = FontId::proportional(10.0);
        let text_color = Color32::WHITE;

        // Measure text
        let galley = painter.layout_no_wrap(text, font_id, text_color);
        let text_size = galley.size();
        let badge_padding = 4.0;
        let badge_width = text_size.x + badge_padding * 2.0;
        let badge_height = text_size.y + badge_padding * 2.0;

        Rect::from_center_size(
            center,
            Vec2::new(badge_width.max(badge_height), badge_height),
        )
    }

    /// Draw a count badge showing the number of bundled edges.
    ///
    /// The badge is a small rounded rectangle with the count displayed in white text,
    /// placed at the midpoint of the bundled edge curve.
    ///
    /// **Validates: Requirements 14.2, Property 30: Edge Bundle Count Badge**
    fn draw_count_badge(
        &self,
        painter: &Painter,
        center: Pos2,
        count: usize,
        badge_color: Color32,
    ) {
        let text = format!("{}", count);
        let font_id = FontId::proportional(10.0);
        let text_color = Color32::WHITE;

        // Measure text
        let galley = painter.layout_no_wrap(text, font_id, text_color);
        let text_size = galley.size();
        let badge_rect = self.compute_badge_rect(painter, center, count);
        let badge_radius = badge_rect.height() / 2.0;

        // Draw badge background (darker version of edge color)
        let bg_color = Color32::from_rgba_unmultiplied(
            (badge_color.r() as f32 * 0.5) as u8,
            (badge_color.g() as f32 * 0.5) as u8,
            (badge_color.b() as f32 * 0.5) as u8,
            220,
        );

        painter.rect_filled(badge_rect, badge_radius, bg_color);
        painter.rect_stroke(
            badge_rect,
            badge_radius,
            Stroke::new(1.0, badge_color),
            egui::StrokeKind::Outside,
        );

        // Draw text centered in badge
        let text_pos = Pos2::new(center.x - text_size.x / 2.0, center.y - text_size.y / 2.0);
        painter.galley(text_pos, galley, text_color);
    }

    fn draw_scaled_arrowhead(
        &self,
        painter: &Painter,
        pos: Pos2,
        dir: codestory_graph::Vec2,
        color: Color32,
        arrow_len: f32,
        arrow_width: f32,
    ) {
        let angle = dir.y.atan2(dir.x);
        let tip = pos;
        let back_center = tip - Vec2::angled(angle) * arrow_len;
        let left = back_center + Vec2::angled(angle + std::f32::consts::FRAC_PI_2) * arrow_width;
        let right = back_center + Vec2::angled(angle - std::f32::consts::FRAC_PI_2) * arrow_width;

        painter.add(Shape::convex_polygon(
            vec![tip, right, left],
            color,
            Stroke::NONE,
        ));
    }
}

fn mix_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let (ar, ag, ab, aa) = a.to_tuple();
    let (br, bg, bb, ba) = b.to_tuple();
    let lerp = |x: u8, y: u8| -> u8 {
        ((x as f32 * (1.0 - t) + y as f32 * t).round() as i32)
            .clamp(0, 255) as u8
    };
    Color32::from_rgba_unmultiplied(
        lerp(ar, br),
        lerp(ag, bg),
        lerp(ab, bb),
        lerp(aa, ba),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_graph::edge_router::CubicBezier;
    use codestory_graph::Vec2 as GVec2;

    #[test]
    fn test_hit_test_near_edge() {
        let mut overlay = EdgeOverlay::new();
        overlay.hit_tolerance = 10.0;

        // Create a straight horizontal edge from (0,0) to (100,0)
        let curve = CubicBezier {
            start: GVec2::new(0.0, 0.0),
            control1: GVec2::new(33.0, 0.0),
            control2: GVec2::new(66.0, 0.0),
            end: GVec2::new(100.0, 0.0),
        };

        overlay.edge_hit_infos.push(EdgeHitInfo {
            kind: EdgeKind::CALL,
            source: NodeId(1),
            target: NodeId(2),
            source_label: "A".to_string(),
            target_label: "B".to_string(),
            screen_curve: curve,
            is_bundled: false,
        });

        // Point on the edge -> should hit
        let result = overlay.hit_test(Pos2::new(50.0, 0.0));
        assert!(result.is_some());
        assert_eq!(result.unwrap(), 0);

        // Point near the edge (within tolerance) -> should hit
        let result = overlay.hit_test(Pos2::new(50.0, 5.0));
        assert!(result.is_some());

        // Point far from the edge -> should miss
        let result = overlay.hit_test(Pos2::new(50.0, 50.0));
        assert!(result.is_none());
    }

    #[test]
    fn test_hit_test_closest_edge() {
        let mut overlay = EdgeOverlay::new();
        overlay.hit_tolerance = 10.0;

        // Two parallel horizontal edges
        let curve1 = CubicBezier {
            start: GVec2::new(0.0, 0.0),
            control1: GVec2::new(33.0, 0.0),
            control2: GVec2::new(66.0, 0.0),
            end: GVec2::new(100.0, 0.0),
        };
        let curve2 = CubicBezier {
            start: GVec2::new(0.0, 20.0),
            control1: GVec2::new(33.0, 20.0),
            control2: GVec2::new(66.0, 20.0),
            end: GVec2::new(100.0, 20.0),
        };

        overlay.edge_hit_infos.push(EdgeHitInfo {
            kind: EdgeKind::CALL,
            source: NodeId(1),
            target: NodeId(2),
            source_label: "A".to_string(),
            target_label: "B".to_string(),
            screen_curve: curve1,
            is_bundled: false,
        });
        overlay.edge_hit_infos.push(EdgeHitInfo {
            kind: EdgeKind::INHERITANCE,
            source: NodeId(3),
            target: NodeId(4),
            source_label: "C".to_string(),
            target_label: "D".to_string(),
            screen_curve: curve2,
            is_bundled: false,
        });

        // Point closer to first edge
        let result = overlay.hit_test(Pos2::new(50.0, 3.0));
        assert_eq!(result, Some(0));

        // Point closer to second edge
        let result = overlay.hit_test(Pos2::new(50.0, 17.0));
        assert_eq!(result, Some(1));
    }

    #[test]
    fn test_highlighted_nodes_updated_on_hover() {
        let overlay = EdgeOverlay::new();
        // Initially no highlighted nodes
        assert!(overlay.highlighted_nodes.is_empty());
    }

    #[test]
    fn test_selected_edge_persistence() {
        let mut overlay = EdgeOverlay::new();
        assert!(overlay.get_selected_edge().is_none());

        overlay.selected_edge = Some(SelectedEdgeInfo {
            kind: EdgeKind::CALL,
            source: NodeId(1),
            target: NodeId(2),
            source_label: "A".to_string(),
            target_label: "B".to_string(),
        });

        assert!(overlay.get_selected_edge().is_some());
        let sel = overlay.get_selected_edge().unwrap();
        assert_eq!(sel.source, NodeId(1));
        assert_eq!(sel.target, NodeId(2));

        overlay.clear_selection();
        assert!(overlay.get_selected_edge().is_none());
    }
}
