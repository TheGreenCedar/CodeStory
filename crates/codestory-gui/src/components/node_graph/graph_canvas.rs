use codestory_core::NodeId;
use codestory_graph::uml_types::{GraphViewState, MemberItem, UmlNode, VisibilityKind};
use eframe::egui;
use egui_phosphor::regular as ph;

use crate::settings::NodeGraphSettings;
use super::style_resolver::StyleResolver;

// Responsibility checklist for the custom canvas:
// - Node cards (header, members, hatching) and collapse/expand interactions
// - Selection, hover, and context menus
// - Edge overlay integration (bundles, tooltips, hit testing)
// - Minimap + legend overlays
// - View state (pan/zoom) persistence

#[derive(Clone, Copy)]
pub struct GraphCanvasNode<'a> {
    pub uml: &'a UmlNode,
    pub pos: egui::Pos2,
}

pub struct GraphCanvasInteraction {
    pub clicked_node: Option<NodeId>,
    #[allow(dead_code)]
    pub hovered_node: Option<NodeId>,
    pub toggle_node: Option<NodeId>,
    pub toggle_section: Option<(NodeId, VisibilityKind)>,
    pub action: Option<GraphCanvasAction>,
}

#[derive(Clone, Copy)]
pub enum GraphCanvasAction {
    Focus(NodeId),
    Navigate(NodeId),
    Hide(NodeId),
    OpenInNewTab(NodeId),
    ShowDefinition(NodeId),
    ShowInCode(NodeId),
    ShowInIde(NodeId),
    Bookmark(NodeId),
    CopyName(NodeId),
    CopyPath(NodeId),
    OpenContainingFolder(NodeId),
    CopyGraphImage,
    ExportGraphImage,
    HistoryBack,
    HistoryForward,
}

pub struct GraphCanvasOutput {
    pub interaction: GraphCanvasInteraction,
    pub node_rects_graph: std::collections::HashMap<NodeId, egui::Rect>,
    pub transform: egui::emath::TSTransform,
    pub edge_overlay_enabled: bool,
    #[allow(dead_code)]
    pub lod_mode: GraphLodMode,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GraphLodMode {
    Detail,
    Simplified,
    PointCloud,
}

struct GridIndex {
    cell_size: f32,
    cells: std::collections::HashMap<(i32, i32), Vec<usize>>,
}

struct SectionLayout<'a> {
    kind: VisibilityKind,
    header_rect: egui::Rect,
    members: Vec<(&'a MemberItem, egui::Rect)>,
}

struct NodeLayout<'a> {
    rect: egui::Rect,
    header_rect: egui::Rect,
    sections: Vec<SectionLayout<'a>>,
}

#[derive(Clone, Copy)]
struct DragState {
    start_pan: egui::Vec2,
    start_pos: egui::Pos2,
}

pub struct GraphCanvas {
    zoom: f32,
    pan: egui::Vec2,
    drag_state: Option<DragState>,
    dragging_node: Option<NodeId>,
}

impl GraphCanvas {
    pub fn new() -> Self {
        Self {
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            drag_state: None,
            dragging_node: None,
        }
    }

    pub fn sync_from_view_state(&mut self, view_state: &GraphViewState) {
        if self.drag_state.is_some() {
            return;
        }
        self.zoom = view_state.zoom;
        self.pan = egui::vec2(view_state.pan.x, view_state.pan.y);
    }

    pub fn apply_to_view_state(&self, view_state: &mut GraphViewState) -> bool {
        let mut changed = false;
        if (view_state.zoom - self.zoom).abs() > f32::EPSILON {
            view_state.zoom = self.zoom;
            changed = true;
        }

        let pan = codestory_graph::Vec2::new(self.pan.x, self.pan.y);
        if view_state.pan != pan {
            view_state.pan = pan;
            changed = true;
        }
        changed
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn zoom_by(&mut self, delta: f32, viewport_center: egui::Pos2) {
        if delta <= 0.0 {
            return;
        }
        let prev_zoom = self.zoom;
        let new_zoom = (self.zoom * delta).clamp(0.1, 4.0);
        if (new_zoom - prev_zoom).abs() <= f32::EPSILON {
            return;
        }
        self.zoom = new_zoom;
        let graph_pos = self.screen_to_graph(viewport_center, viewport_center, prev_zoom);
        let new_screen = self.graph_to_screen(graph_pos, viewport_center);
        self.pan += viewport_center - new_screen;
    }

    pub fn zoom_to_fit(&mut self, bounds: egui::Rect, viewport: egui::Rect, padding: f32) {
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            return;
        }
        let padded = bounds.expand(padding);
        let available = viewport.shrink(padding);
        let scale = (available.width() / padded.width())
            .min(available.height() / padded.height())
            .clamp(0.1, 4.0);
        self.zoom = scale;
        self.pan = -padded.center().to_vec2() * self.zoom;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        nodes: &[GraphCanvasNode<'_>],
        edges: &[codestory_graph::DummyEdge],
        view_state: &GraphViewState,
        custom_positions: &mut std::collections::HashMap<NodeId, codestory_graph::Vec2>,
        settings: &NodeGraphSettings,
        style: &StyleResolver,
    ) -> GraphCanvasOutput {
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        let bg_layer = egui::LayerId::new(egui::Order::Background, ui.layer_id().id);
        let mut bg_painter = ui.ctx().layer_painter(bg_layer);
        bg_painter.set_clip_rect(rect);
        let painter = ui.painter_at(rect);
        let palette = style.palette();
        bg_painter.rect_filled(rect, 0.0, palette.background);
        let viewport_center = rect.center();
        let mut hovered_node = None;
        let mut clicked_node = None;
        let mut toggle_node = None;
        let mut toggle_section = None;
        let mut action = None;

        let zoom_delta = ui.input(|i| i.zoom_delta());
        if response.hovered() && (zoom_delta - 1.0).abs() > f32::EPSILON {
            let prev_zoom = self.zoom;
            let new_zoom = (self.zoom * zoom_delta).clamp(0.1, 4.0);
            if (new_zoom - prev_zoom).abs() > f32::EPSILON {
                self.zoom = new_zoom;
                if let Some(pointer) = response.hover_pos() {
                    let graph_pos = self.screen_to_graph(pointer, viewport_center, prev_zoom);
                    let new_screen = self.graph_to_screen(graph_pos, viewport_center);
                    self.pan += pointer - new_screen;
                }
            }
        }

        let lod_mode = select_lod_mode(self.zoom, nodes.len(), settings);
        let detail_mode = lod_mode == GraphLodMode::Detail;
        let simplified_mode = lod_mode == GraphLodMode::Simplified;
        let hover_radius = if self.zoom < 0.6 { 14.0 } else { 10.0 };
        let pointer_pos = response.hover_pos();
        let mut layouts = std::collections::HashMap::new();
        let transform =
            egui::emath::TSTransform::new(viewport_center.to_vec2() + self.pan, self.zoom);
        let mut node_rects_graph = std::collections::HashMap::new();
        let edge_overlay_enabled = lod_mode == GraphLodMode::Detail;
        let view_rect_graph = transform.inverse().mul_rect(rect);
        let use_grid = nodes.len() >= 2000;
        let grid = if use_grid {
            Some(GridIndex::build(nodes, 200.0))
        } else {
            None
        };
        let visible_indices = visible_nodes(nodes, &grid, view_rect_graph);
        let visible_set: std::collections::HashSet<NodeId> = visible_indices
            .iter()
            .map(|idx| nodes[*idx].uml.id)
            .collect();

        if detail_mode || simplified_mode {
            for idx in &visible_indices {
                let node = &nodes[*idx];
                let collapse_state = if simplified_mode {
                    codestory_graph::uml_types::CollapseState::collapsed()
                } else {
                    view_state.get_collapse_state(node.uml.id)
                };
                let screen_min = self.graph_to_screen(node.pos, viewport_center);
                let layout = layout_node(ui, node, &collapse_state, self.zoom, screen_min);
                layouts.insert(node.uml.id, layout);
            }
            if edge_overlay_enabled {
                for (node_id, layout) in &layouts {
                    node_rects_graph.insert(*node_id, transform.inverse().mul_rect(layout.rect));
                }
            }
        }

        if let Some(pointer) = pointer_pos {
            if detail_mode {
                for (node_id, layout) in &layouts {
                    if layout.rect.contains(pointer) {
                        hovered_node = Some(*node_id);
                        break;
                    }
                }
            } else {
                let search_radius = hover_radius / self.zoom;
                if let Some(grid) = &grid {
                    if let Some(idx) =
                        grid.nearest(nodes, pointer, viewport_center, self, search_radius)
                    {
                        hovered_node = Some(nodes[idx].uml.id);
                    }
                } else {
                    let mut best = hover_radius;
                    for idx in &visible_indices {
                        let node = &nodes[*idx];
                        let screen_pos = self.graph_to_screen(node.pos, viewport_center);
                        let dist = screen_pos.distance(pointer);
                        if dist <= best {
                            best = dist;
                            hovered_node = Some(node.uml.id);
                        }
                    }
                }
            }
        }

        if response.clicked() {
            clicked_node = hovered_node;
        }

        if detail_mode || simplified_mode {
            if response.drag_started() && hovered_node.is_some() {
                self.dragging_node = hovered_node;
            }
            if let Some(node_id) = self.dragging_node {
                if let Some(pointer) = response.interact_pointer_pos() {
                    let graph_pos = self.screen_to_graph(pointer, viewport_center, self.zoom);
                    custom_positions.insert(node_id, codestory_graph::Vec2::new(graph_pos.x, graph_pos.y));
                }
                if ui.input(|i| !i.pointer.primary_down()) {
                    self.dragging_node = None;
                }
            } else if response.drag_started() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    self.drag_state = Some(DragState {
                        start_pan: self.pan,
                        start_pos: pointer,
                    });
                }
            }
        } else if response.drag_started() {
            if let Some(pointer) = response.interact_pointer_pos() {
                self.drag_state = Some(DragState {
                    start_pan: self.pan,
                    start_pos: pointer,
                });
            }
        }

        if response.dragged() && self.dragging_node.is_none() {
            if let (Some(state), Some(pointer)) =
                (self.drag_state, response.interact_pointer_pos())
            {
                self.pan = state.start_pan + (pointer - state.start_pos);
            }
        }
        if self.drag_state.is_some() && ui.input(|i| !i.pointer.primary_down()) {
            self.drag_state = None;
        }

        if detail_mode {
            for node in nodes {
                let Some(layout) = layouts.get(&node.uml.id) else {
                    continue;
                };
                if !rect.intersects(layout.rect) {
                    continue;
                }
                let collapse_state = view_state.get_collapse_state(node.uml.id);
                draw_node_card(ui, &painter, node, &collapse_state, layout, style, self.zoom);

                let header_id = ui.id().with(("graph_node_header", node.uml.id));
                let header_response =
                    ui.interact(layout.header_rect, header_id, egui::Sense::click());
                if header_response.double_clicked() {
                    toggle_node = Some(node.uml.id);
                }
                if header_response.clicked() {
                    clicked_node = Some(node.uml.id);
                }
                header_response.context_menu(|ui| {
                    if ui.button("Open in New Tab").clicked() {
                        action = Some(GraphCanvasAction::OpenInNewTab(node.uml.id));
                        ui.close();
                    }
                    if ui.button("Show Definition").clicked() {
                        action = Some(GraphCanvasAction::ShowDefinition(node.uml.id));
                        ui.close();
                    }
                    if ui.button("Show in Code").clicked() {
                        action = Some(GraphCanvasAction::ShowInCode(node.uml.id));
                        ui.close();
                    }
                    if ui.button("Show in IDE").clicked() {
                        action = Some(GraphCanvasAction::ShowInIde(node.uml.id));
                        ui.close();
                    }
                    if ui.button("Bookmark").clicked() {
                        action = Some(GraphCanvasAction::Bookmark(node.uml.id));
                        ui.close();
                    }
                    if ui.button("Copy Name").clicked() {
                        action = Some(GraphCanvasAction::CopyName(node.uml.id));
                        ui.close();
                    }
                    if ui.button("Copy Path").clicked() {
                        action = Some(GraphCanvasAction::CopyPath(node.uml.id));
                        ui.close();
                    }
                    if ui.button("Open Containing Folder").clicked() {
                        action = Some(GraphCanvasAction::OpenContainingFolder(node.uml.id));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Copy Graph Image").clicked() {
                        action = Some(GraphCanvasAction::CopyGraphImage);
                        ui.close();
                    }
                    if ui.button("Export Graph Image...").clicked() {
                        action = Some(GraphCanvasAction::ExportGraphImage);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Focus").clicked() {
                        action = Some(GraphCanvasAction::Focus(node.uml.id));
                        ui.close();
                    }
                    if let Some(parent_id) = node.uml.parent_id
                        && ui.button("Go to Parent").clicked()
                    {
                        action = Some(GraphCanvasAction::Navigate(parent_id));
                        ui.close();
                    }
                    if ui.button("Back").clicked() {
                        action = Some(GraphCanvasAction::HistoryBack);
                        ui.close();
                    }
                    if ui.button("Forward").clicked() {
                        action = Some(GraphCanvasAction::HistoryForward);
                        ui.close();
                    }
                    if ui.button("Hide").clicked() {
                        action = Some(GraphCanvasAction::Hide(node.uml.id));
                        ui.close();
                    }
                });

                for section in &layout.sections {
                    let section_id =
                        ui.id().with(("graph_node_section", node.uml.id, section.kind));
                    let section_response =
                        ui.interact(section.header_rect, section_id, egui::Sense::click());
                    if section_response.clicked() {
                        toggle_section = Some((node.uml.id, section.kind));
                    }

                    for (member, member_rect) in &section.members {
                        let member_id = ui.id().with(("graph_member", node.uml.id, member.id));
                        let member_response =
                            ui.interact(*member_rect, member_id, egui::Sense::click());
                        if member_response.clicked() {
                            clicked_node = Some(member.id);
                        }
                        member_response.context_menu(|ui| {
                            if ui.button("Open in New Tab").clicked() {
                                action = Some(GraphCanvasAction::OpenInNewTab(member.id));
                                ui.close();
                            }
                            if ui.button("Show Definition").clicked() {
                                action = Some(GraphCanvasAction::ShowDefinition(member.id));
                                ui.close();
                            }
                            if ui.button("Show in Code").clicked() {
                                action = Some(GraphCanvasAction::ShowInCode(member.id));
                                ui.close();
                            }
                            if ui.button("Show in IDE").clicked() {
                                action = Some(GraphCanvasAction::ShowInIde(member.id));
                                ui.close();
                            }
                            if ui.button("Bookmark").clicked() {
                                action = Some(GraphCanvasAction::Bookmark(member.id));
                                ui.close();
                            }
                            if ui.button("Copy Name").clicked() {
                                action = Some(GraphCanvasAction::CopyName(member.id));
                                ui.close();
                            }
                            if ui.button("Copy Path").clicked() {
                                action = Some(GraphCanvasAction::CopyPath(member.id));
                                ui.close();
                            }
                            if ui.button("Open Containing Folder").clicked() {
                                action = Some(GraphCanvasAction::OpenContainingFolder(member.id));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Copy Graph Image").clicked() {
                                action = Some(GraphCanvasAction::CopyGraphImage);
                                ui.close();
                            }
                            if ui.button("Export Graph Image...").clicked() {
                                action = Some(GraphCanvasAction::ExportGraphImage);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Focus").clicked() {
                                action = Some(GraphCanvasAction::Focus(member.id));
                                ui.close();
                            }
                            if ui.button("Go to Definition").clicked() {
                                action = Some(GraphCanvasAction::Navigate(member.id));
                                ui.close();
                            }
                            if ui.button("Back").clicked() {
                                action = Some(GraphCanvasAction::HistoryBack);
                                ui.close();
                            }
                            if ui.button("Forward").clicked() {
                                action = Some(GraphCanvasAction::HistoryForward);
                                ui.close();
                            }
                            if ui.button("Hide Parent").clicked() {
                                action = Some(GraphCanvasAction::Hide(node.uml.id));
                                ui.close();
                            }
                        });
                        member_response.on_hover_ui(|ui| {
                            ui.label(format!("{:?} {}", member.kind, member.name));
                            if let Some(signature) = &member.signature {
                                ui.label(signature);
                            }
                            if member.has_outgoing_edges {
                                ui.label("Has outgoing edges");
                            }
                        });
                    }
                }
            }
        } else if simplified_mode {
            for idx in &visible_indices {
                let node = &nodes[*idx];
                let Some(layout) = layouts.get(&node.uml.id) else {
                    continue;
                };
                if !rect.intersects(layout.rect) {
                    continue;
                }
                let collapse_state = codestory_graph::uml_types::CollapseState::collapsed();
                draw_node_card(ui, &painter, node, &collapse_state, layout, style, self.zoom);
        if hovered_node == Some(node.uml.id) {
            let palette = style.palette();
            painter.rect_stroke(
                layout.rect,
                6.0 * self.zoom,
                egui::Stroke::new(1.5, palette.node_default_text),
                egui::StrokeKind::Middle,
            );
        }
    }
        } else {
            draw_point_cloud(
                &painter,
                nodes,
                &visible_indices,
                viewport_center,
                self,
                style,
                hovered_node,
            );
        }

        if !detail_mode {
            let positions: std::collections::HashMap<NodeId, egui::Pos2> =
                nodes.iter().map(|n| (n.uml.id, n.pos)).collect();
            draw_lod_edges(
                &bg_painter,
                edges,
                &visible_set,
                &positions,
                viewport_center,
                self,
                style,
                lod_mode,
            );
        }

        if settings.show_graph_stats {
            draw_stats_overlay(ui, rect, lod_mode, nodes.len(), visible_indices.len());
        }

        GraphCanvasOutput {
            interaction: GraphCanvasInteraction {
                clicked_node,
                hovered_node,
                toggle_node,
                toggle_section,
                action,
            },
            node_rects_graph,
            transform,
            edge_overlay_enabled,
            lod_mode,
        }
    }

    fn graph_to_screen(&self, graph_pos: egui::Pos2, viewport_center: egui::Pos2) -> egui::Pos2 {
        viewport_center + self.pan + (graph_pos.to_vec2() * self.zoom)
    }

    fn screen_to_graph(
        &self,
        screen_pos: egui::Pos2,
        viewport_center: egui::Pos2,
        zoom: f32,
    ) -> egui::Pos2 {
        let offset = screen_pos - viewport_center - self.pan;
        egui::Pos2::new(offset.x / zoom, offset.y / zoom)
    }
}

impl GridIndex {
    fn build(nodes: &[GraphCanvasNode<'_>], cell_size: f32) -> Self {
        let mut cells: std::collections::HashMap<(i32, i32), Vec<usize>> = std::collections::HashMap::new();
        for (idx, node) in nodes.iter().enumerate() {
            let key = cell_for(node.pos, cell_size);
            cells.entry(key).or_default().push(idx);
        }
        Self { cell_size, cells }
    }

    fn query_rect(&self, rect: egui::Rect) -> Vec<usize> {
        let min = cell_for(rect.min, self.cell_size);
        let max = cell_for(rect.max, self.cell_size);
        let mut result = std::collections::HashSet::new();
        for y in min.1..=max.1 {
            for x in min.0..=max.0 {
                if let Some(list) = self.cells.get(&(x, y)) {
                    for idx in list {
                        result.insert(*idx);
                    }
                }
            }
        }
        result.into_iter().collect()
    }

    fn nearest(
        &self,
        nodes: &[GraphCanvasNode<'_>],
        pointer: egui::Pos2,
        viewport_center: egui::Pos2,
        canvas: &GraphCanvas,
        radius_graph: f32,
    ) -> Option<usize> {
        let point_graph = canvas.screen_to_graph(pointer, viewport_center, canvas.zoom);
        let rect = egui::Rect::from_center_size(
            point_graph,
            egui::vec2(radius_graph * 2.0, radius_graph * 2.0),
        );
        let candidates = self.query_rect(rect);
        let mut best = radius_graph;
        let mut best_idx = None;
        for idx in candidates {
            let node = &nodes[idx];
            let dist = node.pos.distance(point_graph);
            if dist <= best {
                best = dist;
                best_idx = Some(idx);
            }
        }
        best_idx
    }
}

fn cell_for(pos: egui::Pos2, cell_size: f32) -> (i32, i32) {
    let x = (pos.x / cell_size).floor() as i32;
    let y = (pos.y / cell_size).floor() as i32;
    (x, y)
}

fn visible_nodes(
    nodes: &[GraphCanvasNode<'_>],
    grid: &Option<GridIndex>,
    view_rect_graph: egui::Rect,
) -> Vec<usize> {
    let expanded = view_rect_graph.expand(200.0);
    if let Some(grid) = grid {
        grid.query_rect(expanded)
    } else {
        (0..nodes.len()).collect()
    }
}

fn select_lod_mode(
    zoom: f32,
    node_count: usize,
    settings: &NodeGraphSettings,
) -> GraphLodMode {
    if node_count > settings.max_full_nodes {
        if zoom >= settings.lod_simplified_zoom {
            GraphLodMode::Simplified
        } else {
            GraphLodMode::PointCloud
        }
    } else if zoom < settings.lod_points_zoom {
        GraphLodMode::PointCloud
    } else if zoom < settings.lod_simplified_zoom {
        GraphLodMode::Simplified
    } else {
        GraphLodMode::Detail
    }
}

fn draw_point_cloud(
    painter: &egui::Painter,
    nodes: &[GraphCanvasNode<'_>],
    visible_indices: &[usize],
    viewport_center: egui::Pos2,
    canvas: &GraphCanvas,
    style: &StyleResolver,
    hovered_node: Option<NodeId>,
) {
    for idx in visible_indices {
        let node = &nodes[*idx];
        let screen_pos = canvas.graph_to_screen(node.pos, viewport_center);
        let color = style.resolve_node_color(node.uml.kind);
        let radius = 2.5;
        painter.circle_filled(screen_pos, radius, color);
        if hovered_node == Some(node.uml.id) {
            let palette = style.palette();
            painter.circle_stroke(
                screen_pos,
                radius + 3.0,
                egui::Stroke::new(1.5, palette.node_default_text),
            );
        }
    }
}

fn draw_lod_edges(
    painter: &egui::Painter,
    edges: &[codestory_graph::DummyEdge],
    visible_set: &std::collections::HashSet<NodeId>,
    positions: &std::collections::HashMap<NodeId, egui::Pos2>,
    viewport_center: egui::Pos2,
    canvas: &GraphCanvas,
    style: &StyleResolver,
    lod_mode: GraphLodMode,
) {
    let stroke_width =
        if lod_mode == GraphLodMode::PointCloud { 0.6 } else { 1.0 } * style.edge_width_scale();
    for edge in edges {
        if !edge.visible {
            continue;
        }
        if !visible_set.contains(&edge.source) && !visible_set.contains(&edge.target) {
            continue;
        }
        let (Some(source_pos), Some(target_pos)) =
            (positions.get(&edge.source), positions.get(&edge.target))
        else {
            continue;
        };
        let start = canvas.graph_to_screen(*source_pos, viewport_center);
        let end = canvas.graph_to_screen(*target_pos, viewport_center);
        let color = style.resolve_edge_color(edge.kind);
        painter.line_segment([start, end], egui::Stroke::new(stroke_width, color));
    }
}

fn draw_stats_overlay(
    ui: &egui::Ui,
    rect: egui::Rect,
    mode: GraphLodMode,
    total_nodes: usize,
    visible_nodes: usize,
) {
    let label = format!(
        "LOD: {:?}\nNodes: {} (visible {})",
        mode, total_nodes, visible_nodes
    );
    egui::Area::new("graph_stats_overlay".into())
        .fixed_pos(rect.min + egui::vec2(8.0, 8.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            ui.label(label);
        });
}

fn layout_node<'a>(
    ui: &egui::Ui,
    node: &'a GraphCanvasNode<'a>,
    collapse_state: &codestory_graph::uml_types::CollapseState,
    zoom: f32,
    screen_min: egui::Pos2,
) -> NodeLayout<'a> {
    let header_height = 32.0 * zoom;
    let section_header_height = 20.0 * zoom;
    let member_row_height = 22.0 * zoom;
    let padding = 12.0 * zoom;
    let min_width = 170.0 * zoom;
    let header_padding = 12.0 * zoom;
    let pill_padding_x = 6.0 * zoom;

    let header_font = egui::FontId::proportional(13.0 * zoom);
    let section_font = egui::FontId::proportional(9.5 * zoom);
    let member_font = egui::FontId::proportional(10.5 * zoom);

    let mut width = min_width;
    let header_label = format_header_label(node);
    width = width.max(
        measure_text_width(ui, &header_label, &header_font) + header_padding * 2.0 + 16.0 * zoom,
    );

    for section in &node.uml.visibility_sections {
        if section.members.is_empty() {
            continue;
        }
        let section_extra = section_icon_extra_width(section.kind, zoom);
        width = width.max(
            measure_text_width(ui, section.kind.label(), &section_font)
                + section_extra
                + padding * 2.0
                + 8.0 * zoom,
        );
        if !collapse_state.is_section_collapsed(section.kind) && !collapse_state.is_collapsed {
            for member in &section.members {
                let member_label = member_label(member);
                width = width.max(
                    measure_text_width(ui, &member_label, &member_font)
                        + pill_padding_x * 2.0
                        + padding
                        + 8.0 * zoom,
                );
            }
        }
    }

    let mut height = header_height + padding * 0.5;
    let mut sections = Vec::new();

    if !collapse_state.is_collapsed {
        height += padding * 0.5;
        for section in &node.uml.visibility_sections {
            if section.members.is_empty() {
                continue;
            }
            height += section_header_height;
            if !collapse_state.is_section_collapsed(section.kind) {
                height += section.members.len() as f32 * member_row_height;
            }
            height += padding * 0.2;
        }
    }

    let rect = egui::Rect::from_min_size(screen_min, egui::vec2(width, height));
    let header_rect = egui::Rect::from_min_size(rect.min, egui::vec2(width, header_height));

    let mut cursor_y = header_rect.max.y + padding * 0.2;
    if !collapse_state.is_collapsed {
        for section in &node.uml.visibility_sections {
            if section.members.is_empty() {
                continue;
            }
            let section_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + padding * 0.5, cursor_y),
                egui::vec2(width - padding, section_header_height),
            );
            let mut members = Vec::new();
            cursor_y += section_header_height;

            if !collapse_state.is_section_collapsed(section.kind) {
                for member in &section.members {
                    let member_rect = egui::Rect::from_min_size(
                        egui::pos2(rect.min.x + padding * 0.6, cursor_y),
                        egui::vec2(width - padding, member_row_height),
                    );
                    members.push((member, member_rect));
                    cursor_y += member_row_height;
                }
            }
            cursor_y += padding * 0.2;
            sections.push(SectionLayout {
                kind: section.kind,
                header_rect: section_rect,
                members,
            });
        }
    }

    NodeLayout {
        rect,
        header_rect,
        sections,
    }
}

fn draw_node_card(
    ui: &egui::Ui,
    painter: &egui::Painter,
    node: &GraphCanvasNode<'_>,
    collapse_state: &codestory_graph::uml_types::CollapseState,
    layout: &NodeLayout<'_>,
    style: &StyleResolver,
    zoom: f32,
) {
    let palette = style.palette();
    let radius = 10.0 * zoom;
    let shadow_offset = egui::vec2(0.0, 2.0 * zoom);
    painter.rect_filled(layout.rect.translate(shadow_offset), radius, palette.shadow);
    painter.rect_filled(layout.rect, radius, palette.node_default_fill);
    painter.rect_stroke(
        layout.rect,
        radius,
        egui::Stroke::new(1.0, palette.node_default_border),
        egui::StrokeKind::Middle,
    );

    let header_color = style.resolve_node_color(node.uml.kind);
    painter.rect_filled(layout.header_rect, radius, header_color);
    let text_color = style.resolve_text_color(header_color);
    let header_font = egui::FontId::proportional(13.0 * zoom);
    let header_label = format_header_label(node);
    painter.text(
        egui::pos2(layout.header_rect.min.x + 10.0 * zoom, layout.header_rect.center().y),
        egui::Align2::LEFT_CENTER,
        header_label,
        header_font,
        text_color,
    );

    if !node.uml.is_indexed {
        render_hatching_pattern(painter, layout.header_rect, palette.node_hatching);
    }

    if collapse_state.is_collapsed {
        let total_members: usize = node
            .uml
            .visibility_sections
            .iter()
            .map(|s| s.members.len())
            .sum();
        if total_members > 0 {
            let badge = format!("[{}]", total_members);
            painter.text(
                egui::pos2(layout.header_rect.max.x - 10.0 * zoom, layout.header_rect.center().y),
                egui::Align2::RIGHT_CENTER,
                badge,
                egui::FontId::proportional(10.0 * zoom),
                text_color,
            );
        }
        return;
    }

    let section_font = egui::FontId::proportional(9.5 * zoom);
    let member_font = egui::FontId::proportional(10.5 * zoom);

    for section in &layout.sections {
        painter.rect_filled(section.header_rect, radius * 0.5, palette.node_section_fill);
        painter.rect_stroke(
            section.header_rect,
            radius * 0.5,
            egui::Stroke::new(1.0, palette.node_section_border),
            egui::StrokeKind::Middle,
        );

        let label = section.kind.label();
        let label_width = measure_text_width(ui, label, &section_font);
        let chip_padding = egui::vec2(6.0 * zoom, 2.0 * zoom);
        let chip_size = egui::vec2(label_width + chip_padding.x * 2.0, section.header_rect.height() - 4.0 * zoom);
        let chip_rect = egui::Rect::from_min_size(
            egui::pos2(section.header_rect.min.x + 4.0 * zoom, section.header_rect.min.y + 2.0 * zoom),
            chip_size,
        );
        painter.rect_filled(chip_rect, chip_rect.height() / 2.0, palette.section_label_fill);

        let mut text_offset_x = chip_padding.x;
        if let Some(icon_kind) = section_icon_kind(section.kind) {
            let icon_size = 8.0 * zoom;
            let icon_center = egui::pos2(
                chip_rect.min.x + chip_padding.x + icon_size * 0.5,
                chip_rect.center().y,
            );
            let icon_color = style.resolve_icon_color(node.uml.kind);
            match icon_kind {
                SectionIcon::Public => {
                    painter.circle_stroke(
                        icon_center,
                        icon_size * 0.5,
                        egui::Stroke::new(1.2, icon_color),
                    );
                }
                SectionIcon::Private => {
                    let rect = egui::Rect::from_center_size(
                        icon_center,
                        egui::vec2(icon_size * 0.8, icon_size * 0.8),
                    );
                    painter.rect_filled(rect, 2.0 * zoom, icon_color);
                }
            }
            text_offset_x += icon_size + 4.0 * zoom;
        }

        painter.text(
            egui::pos2(chip_rect.min.x + text_offset_x, chip_rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            section_font.clone(),
            palette.section_label_text,
        );

        if !collapse_state.is_section_collapsed(section.kind) {
            for (member, member_rect) in &section.members {
                let member_label = member_label(member);
                let label_width = measure_text_width(ui, &member_label, &member_font);
                let pill_padding = egui::vec2(6.0 * zoom, 3.0 * zoom);
                let pill_height = member_rect.height() - 6.0 * zoom;
                let pill_width = label_width + pill_padding.x * 2.0;
                let pill_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        member_rect.min.x + 4.0 * zoom,
                        member_rect.center().y - pill_height * 0.5,
                    ),
                    egui::vec2(pill_width, pill_height),
                );
                let pill_color = style.resolve_node_color(member.kind);
                let pill_text = style.resolve_text_color(pill_color);
                painter.rect_filled(pill_rect, pill_rect.height() / 2.0, pill_color);
                painter.text(
                    egui::pos2(pill_rect.min.x + pill_padding.x, pill_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    member_label,
                    member_font.clone(),
                    pill_text,
                );

                if member.has_outgoing_edges {
                    painter.text(
                        egui::pos2(
                            member_rect.max.x - 12.0 * zoom,
                            member_rect.min.y + 2.0 * zoom,
                        ),
                        egui::Align2::RIGHT_TOP,
                        ph::ARROW_RIGHT,
                        member_font.clone(),
                        style.resolve_outgoing_edge_indicator_color(),
                    );
                }
            }
        }
    }
}

fn render_hatching_pattern(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let pattern = codestory_graph::style::hatching_pattern();
    let hatch_color = egui::Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        pattern.color.a,
    );

    let angle_rad = pattern.angle.to_radians();
    let cos_angle = angle_rad.cos();
    let sin_angle = angle_rad.sin();
    let diagonal_length = ((rect.width().powi(2) + rect.height().powi(2)).sqrt()).ceil();
    let num_lines = (diagonal_length / pattern.spacing).ceil() as i32;
    for i in -num_lines..=num_lines {
        let offset = i as f32 * pattern.spacing;
        let start = egui::Pos2::new(rect.min.x + offset, rect.min.y);
        let end = egui::Pos2::new(
            rect.min.x + offset + diagonal_length * cos_angle,
            rect.min.y + diagonal_length * sin_angle,
        );
        painter.line_segment(
            [start, end],
            egui::Stroke::new(pattern.line_width, hatch_color),
        );
    }
}

#[derive(Clone, Copy)]
enum SectionIcon {
    Public,
    Private,
}

fn section_icon_kind(kind: VisibilityKind) -> Option<SectionIcon> {
    match kind {
        VisibilityKind::Public => Some(SectionIcon::Public),
        VisibilityKind::Private => Some(SectionIcon::Private),
        _ => None,
    }
}

fn section_icon_extra_width(kind: VisibilityKind, zoom: f32) -> f32 {
    match kind {
        VisibilityKind::Public | VisibilityKind::Private => 12.0 * zoom,
        _ => 0.0,
    }
}

fn member_label(member: &MemberItem) -> String {
    if let Some(signature) = &member.signature {
        format!("{}{}", member.name, signature)
    } else {
        member.name.clone()
    }
}

fn measure_text_width(ui: &egui::Ui, text: &str, font_id: &egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_string(), font_id.clone(), egui::Color32::WHITE)
        .size()
        .x
}

fn format_header_label(node: &GraphCanvasNode<'_>) -> String {
    if let Some(bundle) = &node.uml.bundle_info {
        format!("{} ({})", node.uml.label, bundle.count)
    } else {
        node.uml.label.clone()
    }
}
