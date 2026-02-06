use super::graph_canvas::{GraphCanvas, GraphCanvasAction, GraphCanvasNode};
use crate::components::node_graph::edge_overlay::EdgeOverlay;
use crate::components::node_graph::style_resolver::StyleResolver;
use crate::settings::ThemeMode;
use codestory_core::{Edge, EdgeKind, Node, NodeId, NodeKind, TrailConfig, TrailDirection};
use codestory_events::Event;
use codestory_graph::converter::NodeGraphConverter;
use codestory_graph::uml_types::{
    CollapseState, GraphViewState, MemberItem, UmlNode, VisibilityKind,
};
use codestory_graph::{
    DummyEdge, ForceDirectedLayouter, GraphModel, GridLayouter, Layouter, NestingLayouter,
    RadialLayouter,
};
use codestory_storage::Storage;
use eframe::egui;
use egui_phosphor::regular as ph;
use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

type LayoutRequest = (
    Option<NodeId>,
    Vec<Node>,
    Vec<Edge>,
    codestory_events::LayoutAlgorithm,
    codestory_core::LayoutDirection,
);

type EdgeKey = (NodeId, NodeId, EdgeKind);

#[derive(Clone)]
struct EdgeContextMenu {
    pos: egui::Pos2,
    info: crate::components::node_graph::edge_overlay::EdgeHitInfo,
}

pub struct NodeGraphView {
    style_resolver: StyleResolver,
    event_bus: codestory_events::EventBus,
    edge_overlay: EdgeOverlay,
    current_edges: Vec<DummyEdge>,
    graph_canvas: GraphCanvas,

    uml_nodes: Vec<UmlNode>,
    fallback_positions: HashMap<NodeId, egui::Pos2>,
    node_lookup: HashMap<NodeId, Node>,
    hidden_edge_keys: HashSet<(NodeId, NodeId, EdgeKind)>,
    edge_context_menu: Option<EdgeContextMenu>,
    last_node_rects_graph: HashMap<NodeId, egui::Rect>,
    last_view_state: GraphViewState,

    // Data Cache for rebuilding
    cached_data: Option<(NodeId, Vec<Node>, Vec<Edge>)>,
    cached_positions: Option<HashMap<NodeId, egui::Pos2>>,

    // Async Layout
    layout_tx: Sender<LayoutRequest>,
    layout_rx: Receiver<HashMap<NodeId, egui::Pos2>>,
    is_calculating: bool,
    anim_start_positions: Option<HashMap<NodeId, egui::Pos2>>,
    anim_target_positions: Option<HashMap<NodeId, egui::Pos2>>,
    anim_started_at: Option<Instant>,
    anim_duration: Duration,

    // Local Filter State
    hidden_nodes: std::collections::HashSet<NodeId>,

    // State tracking
    last_auto_layout: bool,
    current_layout_algorithm: codestory_events::LayoutAlgorithm,
    current_layout_direction: codestory_core::LayoutDirection,
    _last_settings_version: u64, // To track changes (reserved for future use)

    // UI Features
    pub pending_pan_to_node: Option<NodeId>,
    view_version: u64,
    initial_layout_done: bool,
    current_transform: egui::emath::TSTransform,
    current_zoom: f32,
    clicked_node: Option<NodeId>,
    node_to_focus: Option<NodeId>,
    node_to_hide: Option<NodeId>,
    node_to_navigate: Option<NodeId>,
    pending_zoom_to_fit: bool,
    pending_zoom_steps: i32,
    pending_zoom_reset: bool,
    pending_expand_all: bool,
    pending_collapse_all: bool,
    show_toolbar_panel: bool,
}

pub struct NodeGraphViewResponse {
    pub clicked_node: Option<NodeId>,
    pub view_state_dirty: bool,
}

impl NodeGraphView {
    pub fn new(event_bus: codestory_events::EventBus) -> Self {
        let (req_tx, req_rx) = channel::<(
            Option<NodeId>,
            Vec<Node>,
            Vec<Edge>,
            codestory_events::LayoutAlgorithm,
            codestory_core::LayoutDirection,
        )>();
        let (res_tx, res_rx) = channel::<HashMap<NodeId, egui::Pos2>>();

        // Spawn layout worker
        thread::spawn(move || {
            while let Ok((root, nodes, edges, algorithm, direction)) = req_rx.recv() {
                let mut model = GraphModel::new();
                model.root = root;
                for node in &nodes {
                    model.add_node(node.clone());
                }
                for edge in &edges {
                    model.add_edge(edge.clone());
                }

                let positions = match algorithm {
                    codestory_events::LayoutAlgorithm::ForceDirected => {
                        let mut layouter = ForceDirectedLayouter::default();
                        let mut stages = Vec::new();
                        let final_iterations = layouter.iterations;
                        let iteration_steps = [30, 80, 160, final_iterations];
                        for &iterations in &iteration_steps {
                            layouter.iterations = iterations.max(1);
                            stages.push(layouter.execute(&model));
                        }
                        if stages.len() > 1 {
                            for positions in stages.iter().take(stages.len() - 1) {
                                let mut result = HashMap::new();
                                for (idx, (x, y)) in positions {
                                    if let Some(node) = model.graph.node_weight(*idx) {
                                        result.insert(node.id, egui::pos2(*x, *y));
                                    }
                                }
                                let _ = res_tx.send(result);
                            }
                        }
                        stages.pop().unwrap_or_default()
                    }
                    codestory_events::LayoutAlgorithm::Radial => {
                        let layouter = RadialLayouter::default();
                        layouter.execute(&model)
                    }
                    codestory_events::LayoutAlgorithm::Grid => {
                        let layouter = GridLayouter { spacing: 200.0 };
                        layouter.execute(&model)
                    }
                    codestory_events::LayoutAlgorithm::Hierarchical => {
                        let layouter = NestingLayouter {
                            inner_padding: 20.0,
                            child_spacing: 20.0,
                            direction,
                        };
                        layouter.execute(&model)
                    }
                };

                let mut result = HashMap::new();
                for (idx, (x, y)) in positions {
                    if let Some(node) = model.graph.node_weight(idx) {
                        result.insert(node.id, egui::pos2(x, y));
                    }
                }

                let _ = res_tx.send(result);
            }
        });

        Self {
            style_resolver: StyleResolver::new(ThemeMode::Bright),
            event_bus,
            edge_overlay: EdgeOverlay::new(),
            current_edges: Vec::new(),
            graph_canvas: GraphCanvas::new(),
            uml_nodes: Vec::new(),
            fallback_positions: HashMap::new(),
            node_lookup: HashMap::new(),
            hidden_edge_keys: HashSet::new(),
            edge_context_menu: None,
            last_node_rects_graph: HashMap::new(),
            last_view_state: GraphViewState::default(),
            cached_data: None,
            cached_positions: None,
            layout_tx: req_tx,
            layout_rx: res_rx,
            is_calculating: false,
            anim_start_positions: None,
            anim_target_positions: None,
            anim_started_at: None,
            anim_duration: Duration::from_millis(0),
            // show_classes: true,
            // show_functions: true,
            // show_variables: true,
            hidden_nodes: std::collections::HashSet::new(),
            last_auto_layout: true,
            current_layout_algorithm: codestory_events::LayoutAlgorithm::default(),
            current_layout_direction: codestory_core::LayoutDirection::default(),
            _last_settings_version: 0,
            // show_minimap: true,
            // show_legend: false,
            pending_pan_to_node: None,
            view_version: 0,
            initial_layout_done: false,
            current_transform: egui::emath::TSTransform::default(),
            current_zoom: 1.0,
            clicked_node: None,
            node_to_focus: None,
            node_to_hide: None,
            node_to_navigate: None,
            pending_zoom_to_fit: false,
            pending_zoom_steps: 0,
            pending_zoom_reset: false,
            pending_expand_all: false,
            pending_collapse_all: false,
            show_toolbar_panel: false,
        }
    }

    pub fn handle_event(
        &mut self,
        event: &Event,
        storage: &Option<Storage>,
        settings: &crate::settings::NodeGraphSettings,
    ) {
        match event {
            Event::TrailModeEnter { root_id } => {
                if let Some(storage) = storage {
                    self.load_node_neighborhood(*root_id, storage, settings);
                }
            }
            Event::TrailConfigChange {
                depth,
                direction,
                edge_filter,
            } => {
                if let Some((root_id, _, _)) = &self.cached_data {
                    let root_id = *root_id;
                    if let Some(storage) = storage {
                        let trail_config = codestory_core::TrailConfig {
                            root_id,
                            depth: *depth,
                            direction: *direction,
                            edge_filter: edge_filter.clone(),
                            max_nodes: 500,
                        };

                        if let Ok(result) = storage.get_trail(&trail_config) {
                            self.load_from_data(root_id, &result.nodes, &result.edges, settings);
                        }
                    }
                }
            }
            Event::ExpandAll => {
                self.pending_expand_all = true;
            }
            Event::CollapseAll => {
                self.pending_collapse_all = true;
            }
            Event::ZoomToFit => {
                self.pending_zoom_to_fit = true;
            }
            Event::ZoomIn => {
                self.pending_zoom_steps += 1;
            }
            Event::ZoomOut => {
                self.pending_zoom_steps -= 1;
            }
            Event::ZoomReset => {
                self.pending_zoom_reset = true;
            }
            _ => {}
        }
    }

    pub fn load_node_neighborhood(
        &mut self,
        center_node_id: NodeId,
        storage: &Storage,
        settings: &crate::settings::NodeGraphSettings,
    ) {
        let trail_config = TrailConfig {
            root_id: center_node_id,
            depth: settings.trail_depth,
            direction: settings.trail_direction,
            edge_filter: settings.trail_edge_filter.clone(),
            max_nodes: 500,
        };

        let (nodes, edges) = if let Ok(result) = storage.get_trail(&trail_config) {
            (result.nodes, result.edges)
        } else if let Ok((nodes, edges)) = storage.get_neighborhood(center_node_id) {
            (nodes, edges)
        } else {
            return;
        };

        self.load_from_data(center_node_id, &nodes, &edges, settings);
    }

    pub fn load_from_data(
        &mut self,
        center_node_id: NodeId,
        nodes: &[Node],
        edges: &[Edge],
        settings: &crate::settings::NodeGraphSettings,
    ) {
        self.cached_data = Some((center_node_id, nodes.to_vec(), edges.to_vec()));
        self.cached_positions = None; // Force re-layout on new data

        // Sync local state with settings
        self.current_layout_direction = settings.layout_direction;
        self.current_layout_algorithm = settings.layout_algorithm;

        self.rebuild_graph(settings);
    }

    fn rebuild_graph(&mut self, settings: &crate::settings::NodeGraphSettings) {
        let Some((center_node_id, nodes, edges)) = &self.cached_data else {
            self.uml_nodes.clear();
            self.fallback_positions.clear();
            self.current_edges.clear();
            self.node_lookup.clear();
            return;
        };

        let center_node_id = *center_node_id;
        self.node_lookup = nodes.iter().cloned().map(|node| (node.id, node)).collect();

        // Build member -> parent map from MEMBER edges so we can keep members
        // visible for structural parents even when filters hide their kinds.
        let mut member_parent: HashMap<NodeId, NodeId> = HashMap::new();
        for edge in edges {
            if edge.kind == EdgeKind::MEMBER {
                let (source, target) = edge.effective_endpoints();
                member_parent.entry(target).or_insert(source);
            }
        }

        let is_structural_kind = |kind: NodeKind| {
            matches!(
                kind,
                NodeKind::CLASS
                    | NodeKind::STRUCT
                    | NodeKind::INTERFACE
                    | NodeKind::UNION
                    | NodeKind::ENUM
                    | NodeKind::MODULE
                    | NodeKind::NAMESPACE
                    | NodeKind::PACKAGE
            )
        };

        let mut allowed_structural: HashSet<NodeId> = HashSet::new();
        for node in nodes {
            if self.hidden_nodes.contains(&node.id) {
                continue;
            }
            if node.id == center_node_id {
                allowed_structural.insert(node.id);
                continue;
            }
            if is_structural_kind(node.kind) {
                let include = match node.kind {
                    NodeKind::CLASS
                    | NodeKind::STRUCT
                    | NodeKind::INTERFACE
                    | NodeKind::UNION
                    | NodeKind::ENUM => settings.show_classes,
                    _ => true,
                };
                if include {
                    allowed_structural.insert(node.id);
                }
            }
        }

        // Filter nodes
        let filtered_nodes: Vec<Node> = nodes
            .iter()
            .filter(|n| {
                if n.id == center_node_id {
                    return true;
                } // Always show center
                if self.hidden_nodes.contains(&n.id) {
                    return false;
                }
                if let Some(parent) = member_parent.get(&n.id) {
                    if allowed_structural.contains(parent) {
                        return true;
                    }
                }
                match n.kind {
                    NodeKind::CLASS
                    | NodeKind::STRUCT
                    | NodeKind::INTERFACE
                    | NodeKind::UNION
                    | NodeKind::ENUM => settings.show_classes,
                    NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO => {
                        settings.show_functions
                    }
                    NodeKind::VARIABLE
                    | NodeKind::FIELD
                    | NodeKind::GLOBAL_VARIABLE
                    | NodeKind::CONSTANT
                    | NodeKind::ENUM_CONSTANT => settings.show_variables,
                    _ => true,
                }
            })
            .cloned()
            .collect();

        let filtered_ids: Vec<NodeId> = filtered_nodes.iter().map(|n| n.id).collect();

        let effective_edges: Vec<Edge> = edges
            .iter()
            .map(|edge| edge.with_effective_endpoints())
            .collect();

        let filtered_edges: Vec<Edge> = effective_edges
            .iter()
            .filter(|e| filtered_ids.contains(&e.source) && filtered_ids.contains(&e.target))
            .cloned()
            .collect();

        // Apply bundling
        let mut model = GraphModel::new();
        for node in &filtered_nodes {
            model.add_node(node.clone());
        }
        for edge in &filtered_edges {
            model.add_edge(edge.clone());
        }

        // Rebuild parent-child relationships from MEMBER edges
        model.rebuild_hierarchy();

        // Threshold of 3 means nodes with 3+ connections to the same target get bundled
        let bundler = codestory_graph::NodeBundler::new(3);
        bundler.execute(&mut model);

        // Re-extract nodes and edges after bundling
        let (bundled_nodes, bundled_edges) = model.get_dummy_data();

        tracing::debug!(
            "Rebuilding node graph: {} nodes, {} edges (after bundling)",
            bundled_nodes.len(),
            bundled_edges.len()
        );

        if bundled_nodes.is_empty() {
            return;
        }

        // Trigger async layout calculation if needed and not already cached or calculating
        if settings.auto_layout && self.cached_positions.is_none() && !self.is_calculating {
            self.is_calculating = true;
            let _ = self.layout_tx.send((
                Some(center_node_id),
                filtered_nodes.clone(),
                filtered_edges.clone(),
                self.current_layout_algorithm,
                self.current_layout_direction,
            ));
        }

        // Build Snarl nodes immediately using new UmlNode converter
        let converter = NodeGraphConverter::new();
        let (uml_nodes, _graph_edges, _pin_info) =
            converter.convert_dummies_to_uml(&bundled_nodes, &bundled_edges);

        // Store edges for EdgeOverlay, remapping member endpoints to visible hosts.
        let mut parent_map = HashMap::new();
        for node in &bundled_nodes {
            if let Some(parent) = node.parent {
                parent_map.insert(node.id, parent);
            }
        }
        let visible_ids: HashSet<NodeId> = uml_nodes.iter().map(|n| n.id).collect();

        let resolve_visible = |start: NodeId| -> Option<NodeId> {
            let mut current = start;
            let mut guard = 0;
            loop {
                if visible_ids.contains(&current) {
                    return Some(current);
                }
                if let Some(parent) = parent_map.get(&current) {
                    current = *parent;
                } else {
                    return None;
                }
                guard += 1;
                if guard > 64 {
                    return None;
                }
            }
        };

        let mut overlay_edges = Vec::new();
        for edge in &bundled_edges {
            if edge.kind == EdgeKind::MEMBER {
                continue;
            }
            let source = match resolve_visible(edge.source) {
                Some(id) => id,
                None => continue,
            };
            let target = match resolve_visible(edge.target) {
                Some(id) => id,
                None => continue,
            };
            let key = (source, target, edge.kind);
            if self.hidden_edge_keys.contains(&key) {
                continue;
            }
            let mut edge = edge.clone();
            edge.source = source;
            edge.target = target;
            overlay_edges.push(edge);
        }

        self.current_edges = overlay_edges;

        self.uml_nodes = uml_nodes;
        self.fallback_positions.clear();

        for (i, node) in self.uml_nodes.iter().enumerate() {
            let id = node.id;
            let pos = if let Some(positions) = &self.cached_positions {
                positions.get(&id).copied().unwrap_or_else(|| {
                    if id == center_node_id {
                        egui::pos2(0.0, 0.0)
                    } else {
                        let angle = (i as f32)
                            * (std::f32::consts::PI * 2.0 / (bundled_nodes.len() as f32));
                        let radius = 250.0;
                        egui::pos2(angle.cos() * radius, angle.sin() * radius)
                    }
                })
            } else if id == center_node_id {
                egui::pos2(0.0, 0.0)
            } else {
                let angle =
                    (i as f32) * (std::f32::consts::PI * 2.0 / (bundled_nodes.len() as f32));
                let radius = 250.0;
                egui::pos2(angle.cos() * radius, angle.sin() * radius)
            };

            self.fallback_positions.insert(id, pos);
        }

        self.last_auto_layout = settings.auto_layout;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut crate::settings::NodeGraphSettings,
        theme: ThemeMode,
        animation_speed: f32,
    ) -> NodeGraphViewResponse {
        self.style_resolver.set_theme_mode(theme);
        self.graph_canvas.sync_from_view_state(&settings.view_state);

        let node_inner_margin = egui::Margin {
            left: 16,
            right: 16,
            top: 8,
            bottom: 8,
        };

        let mut node_to_navigate = None;
        let mut view_state_dirty = false;
        let mut copy_text: Option<String> = None;
        let mut copy_image: Option<egui::ColorImage> = None;
        let mut export_image = false;
        let mut open_folder: Option<String> = None;
        let mut open_in_ide: Option<String> = None;
        let mut scroll_to: Option<(String, u32)> = None;
        let mut show_info: Option<String> = None;

        if let Some(node_id) = self.node_to_focus {
            self.node_to_focus = None;
            node_to_navigate = Some(node_id);
        }

        if let Some(node_id) = self.node_to_navigate {
            self.node_to_navigate = None;
            node_to_navigate = Some(node_id);
        }

        if let Some(node_id) = self.node_to_hide {
            self.node_to_hide = None;
            self.hidden_nodes.insert(node_id);
            self.rebuild_graph(settings);
        }

        self.clicked_node = None;

        if self.pending_expand_all {
            self.pending_expand_all = false;
            for node in &self.uml_nodes {
                let mut state = settings.view_state.get_collapse_state(node.id);
                state.is_collapsed = false;
                state.expand_all_sections();
                settings.view_state.set_collapse_state(node.id, state);
            }
            view_state_dirty = true;
        }

        if self.pending_collapse_all {
            self.pending_collapse_all = false;
            for node in &self.uml_nodes {
                let mut state = settings.view_state.get_collapse_state(node.id);
                state.is_collapsed = true;
                settings.view_state.set_collapse_state(node.id, state);
            }
            view_state_dirty = true;
        }

        // Poll for layout results
        // Apply Layout Results
        while let Ok(positions) = self.layout_rx.try_recv() {
            self.is_calculating = false;
            let anim_speed = animation_speed.max(0.0);
            let duration_ms = if anim_speed <= 0.01 {
                0
            } else {
                (600.0 / anim_speed).round() as u64
            };
            if duration_ms == 0 {
                self.cached_positions = Some(positions.clone());
                self.anim_start_positions = None;
                self.anim_target_positions = None;
                self.anim_started_at = None;
                self.anim_duration = Duration::from_millis(0);
            } else {
                let start = self
                    .cached_positions
                    .clone()
                    .unwrap_or_else(|| positions.clone());
                self.anim_start_positions = Some(start);
                self.anim_target_positions = Some(positions);
                self.anim_started_at = Some(Instant::now());
                self.anim_duration = Duration::from_millis(duration_ms);
            }
            if !self.initial_layout_done {
                self.initial_layout_done = true;
                self.view_version = self.view_version.wrapping_add(1);
            }
        }

        if let (Some(start), Some(target), Some(started)) = (
            &self.anim_start_positions,
            &self.anim_target_positions,
            self.anim_started_at,
        ) {
            if self.anim_duration.as_millis() == 0 {
                self.cached_positions = Some(target.clone());
                self.anim_start_positions = None;
                self.anim_target_positions = None;
                self.anim_started_at = None;
            } else {
                let elapsed = started.elapsed();
                let t = (elapsed.as_secs_f32() / self.anim_duration.as_secs_f32()).clamp(0.0, 1.0);
                let eased = t * (2.0 - t);
                let mut interpolated = HashMap::new();
                for (id, target_pos) in target {
                    let start_pos = start.get(id).copied().unwrap_or(*target_pos);
                    let pos = egui::pos2(
                        start_pos.x + (target_pos.x - start_pos.x) * eased,
                        start_pos.y + (target_pos.y - start_pos.y) * eased,
                    );
                    interpolated.insert(*id, pos);
                }
                for (id, custom) in &settings.view_state.custom_positions {
                    interpolated.insert(*id, egui::pos2(custom.x, custom.y));
                }
                self.cached_positions = Some(interpolated);
                if t >= 1.0 {
                    self.anim_start_positions = None;
                    self.anim_target_positions = None;
                    self.anim_started_at = None;
                } else {
                    ui.ctx().request_repaint();
                }
            }
        }

        if self.is_calculating {
            ui.ctx().request_repaint(); // Keep animating while calculating
        }

        let mut rebuild_needed = false;

        // Check settings change
        let settings_changed = self.last_auto_layout != settings.auto_layout
            || self.current_layout_algorithm != settings.layout_algorithm
            || self.current_layout_direction != settings.layout_direction;

        // Note: Filter changes are handled by rebuild_graph called below if needed
        // But we need to detect if we should force a rebuild due to external setting changes

        // Simple check: if settings indicate a filter change that contradicts our current graph,
        // we might rely on the fact that rebuild_graph checks settings.
        // But we need to know IF we should call rebuild_graph.
        // For now, let's assume if settings passed in differ from what we cached, we rebuild.

        if settings_changed {
            // Expanded check logic could go here
            self.cached_positions = None;
            rebuild_needed = true;
        }

        // Check for filter changes by comparing against... we don't store previous filter state easily
        // except implicitly. Let's just trust the caller or check specific fields if we want optimization.
        // Actually, we should probably track a version of settings or check fields.
        // For simplicity/robustness, we can check basic equality of relevant fields if we stored them,
        // or just rely on manual trigger. But wait, user interactions change settings -> new settings passed in -> we need to react.
        // Let's add specific checks for filters to trigger rebuild.
        // Since we don't store "last_show_classes", we might miss it.
        // TODO: Store last filter state to unnecessary rebuilds?
        // Actually, let's just use the `rebuild_needed` flag from the UI interactions below,
        // AND check if `settings` has changed from outside (e.g. reset).
        // For now, relies on UI returns or explicit refetch.
        // But wait, the `settings` passed here are the SOURCE OF TRUTH.
        // We should rebuild if they differ from what we rendered.
        // Since we don't save what we rendered, we might over-render.
        // Optimization: checking specific fields would require saving them in `NodeGraphView`.
        // Let's just assume if `settings` instance changes we might need update? No, that's every frame.
        // Let's trust the boolean flags from UI interactions for now, and maybe add a "force refresh" if needed.
        // Actually, better: Store the `NodeGraphSettings` we used last time.
        // But `NodeGraphSettings` is not `PartialEq`.
        // Let's stick to the current logic: UI interaction triggers rebuild.
        // AND: if we detect a change in `auto_layout` etc we rebuild.

        if self.last_auto_layout != settings.auto_layout {
            self.cached_positions = None;
            rebuild_needed = true;
        }

        let full_rect = ui.available_rect_before_wrap();
        // Force the ui to take up the whole remaining space
        ui.allocate_rect(full_rect, egui::Sense::hover());

        let toolbar_width = 56.0;
        let toolbar_rect =
            egui::Rect::from_min_size(full_rect.min, egui::vec2(toolbar_width, full_rect.height()));
        let graph_rect = egui::Rect::from_min_max(
            egui::pos2(full_rect.min.x + toolbar_width, full_rect.min.y),
            full_rect.max,
        );

        // Left: Vertical toolbar (Req 9.1)
        ui.scope_builder(egui::UiBuilder::new().max_rect(toolbar_rect), |ui| {
            rebuild_needed |= self.ui_toolbar(ui, settings);
        });

        // Draw separator line manually
        ui.painter().vline(
            toolbar_rect.max.x,
            full_rect.y_range(),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );

        // Right: Graph viewport
        ui.scope_builder(egui::UiBuilder::new().max_rect(graph_rect), |ui| {
            let snarl_rect = ui.available_rect_before_wrap();
            let mut child_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(snarl_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );

            let positions = self.build_positions(settings);
            let mut nodes = Vec::new();
            let mut labels = HashMap::new();
            for uml in &self.uml_nodes {
                labels.insert(uml.id, uml.label.clone());
                if let Some(pos) = positions.get(&uml.id) {
                    nodes.push(GraphCanvasNode { uml, pos: *pos });
                }
            }

            if self.pending_zoom_steps != 0 {
                let steps = self.pending_zoom_steps;
                self.pending_zoom_steps = 0;
                let zoom_delta = if steps > 0 { 1.15 } else { 1.0 / 1.15 };
                for _ in 0..steps.abs() {
                    self.graph_canvas.zoom_by(zoom_delta, snarl_rect.center());
                }
            }

            if self.pending_zoom_to_fit {
                self.pending_zoom_to_fit = false;
                if let Some(bounds) = self.positions_bounds(&positions) {
                    self.graph_canvas.zoom_to_fit(bounds, snarl_rect, 24.0);
                }
            }

            if self.pending_zoom_reset {
                self.pending_zoom_reset = false;
                let current = self.graph_canvas.zoom();
                if (current - 1.0).abs() > f32::EPSILON {
                    self.graph_canvas
                        .zoom_by(1.0 / current, snarl_rect.center());
                }
            }

            let view_state_snapshot = settings.view_state.clone();
            let graph_settings = settings.clone();
            let output = self.graph_canvas.show(
                &mut child_ui,
                snarl_rect,
                &nodes,
                &self.current_edges,
                &view_state_snapshot,
                &mut settings.view_state.custom_positions,
                &graph_settings,
                &self.style_resolver,
            );
            self.current_zoom = self.graph_canvas.zoom();
            self.current_transform = output.transform;
            self.last_node_rects_graph = output.node_rects_graph.clone();
            self.last_view_state = view_state_snapshot.clone();
            if self
                .graph_canvas
                .apply_to_view_state(&mut settings.view_state)
            {
                view_state_dirty = true;
            }
            if let Some(node_id) = output.interaction.toggle_node {
                let mut state = settings.view_state.get_collapse_state(node_id);
                state.toggle_collapsed();
                settings.view_state.set_collapse_state(node_id, state);
                view_state_dirty = true;
            }
            if let Some((node_id, kind)) = output.interaction.toggle_section {
                let mut state = settings.view_state.get_collapse_state(node_id);
                state.toggle_section(kind);
                settings.view_state.set_collapse_state(node_id, state);
                view_state_dirty = true;
            }
            if output.interaction.clicked_node.is_some() {
                self.clicked_node = output.interaction.clicked_node;
            }
            if let Some(action) = output.interaction.action {
                match action {
                    GraphCanvasAction::Focus(id) => self.node_to_focus = Some(id),
                    GraphCanvasAction::Navigate(id) => self.node_to_navigate = Some(id),
                    GraphCanvasAction::Hide(id) => self.node_to_hide = Some(id),
                    GraphCanvasAction::OpenInNewTab(id) => {
                        self.event_bus
                            .publish(Event::TabOpen { token_id: Some(id) });
                    }
                    GraphCanvasAction::ShowDefinition(id) => {
                        self.event_bus.publish(Event::ActivateNode {
                            id,
                            origin: codestory_events::ActivationOrigin::Graph,
                        });
                    }
                    GraphCanvasAction::ShowInCode(id) => {
                        let path = self.node_file_path(id);
                        let line = self.node_start_line(id).unwrap_or(1);
                        if let Some(path) = path {
                            scroll_to = Some((path, line));
                        } else {
                            self.event_bus.publish(Event::ActivateNode {
                                id,
                                origin: codestory_events::ActivationOrigin::Graph,
                            });
                        }
                    }
                    GraphCanvasAction::ShowInIde(id) => {
                        if let Some(path) = self.node_file_path(id) {
                            open_in_ide = Some(path);
                        } else {
                            show_info = Some("No file path available for this node.".to_string());
                        }
                    }
                    GraphCanvasAction::Bookmark(id) => {
                        self.event_bus
                            .publish(Event::BookmarkAddDefault { node_id: id });
                    }
                    GraphCanvasAction::CopyName(id) => {
                        let name = self
                            .node_display_name(id)
                            .unwrap_or_else(|| format!("Node {}", id.0));
                        copy_text = Some(name);
                    }
                    GraphCanvasAction::CopyPath(id) => {
                        if let Some(path) = self.node_file_path(id) {
                            copy_text = Some(path);
                        } else {
                            show_info = Some("No file path available for this node.".to_string());
                        }
                    }
                    GraphCanvasAction::OpenContainingFolder(id) => {
                        if let Some(path) = self.node_file_path(id) {
                            open_folder = Some(path);
                        } else {
                            show_info = Some("No file path available for this node.".to_string());
                        }
                    }
                    GraphCanvasAction::CopyGraphImage => {
                        copy_image = self.build_graph_image();
                        if copy_image.is_none() {
                            show_info = Some("No graph data available to copy.".to_string());
                        }
                    }
                    GraphCanvasAction::ExportGraphImage => {
                        export_image = true;
                    }
                    GraphCanvasAction::HistoryBack => {
                        self.event_bus.publish(Event::HistoryBack);
                    }
                    GraphCanvasAction::HistoryForward => {
                        self.event_bus.publish(Event::HistoryForward);
                    }
                }
            }

            // Render edges via EdgeOverlay using GraphCanvas rects and transform
            if output.edge_overlay_enabled {
                let bg_layer = egui::LayerId::new(egui::Order::Background, ui.layer_id().id);
                let mut bg_painter = ui.ctx().layer_painter(bg_layer);
                bg_painter.set_clip_rect(snarl_rect);
                self.edge_overlay.set_node_labels(labels);
                self.edge_overlay.render(
                    ui,
                    &bg_painter,
                    &bg_painter,
                    &self.current_edges,
                    &output.node_rects_graph,
                    snarl_rect,
                    output.transform,
                    node_inner_margin,
                    &self.style_resolver,
                );
            } else {
                self.edge_overlay.clear_frame_state();
            }

            if ui.input(|i| i.pointer.secondary_clicked())
                && ui
                    .input(|i| i.pointer.interact_pos())
                    .is_some_and(|pos| snarl_rect.contains(pos))
            {
                if let Some(info) = self.edge_overlay.hovered_edge_info().cloned() {
                    let pos = ui
                        .input(|i| i.pointer.interact_pos())
                        .unwrap_or(snarl_rect.center());
                    self.edge_context_menu = Some(EdgeContextMenu { pos, info });
                } else {
                    self.edge_context_menu = None;
                }
            }

            if let Some(menu) = self.edge_context_menu.clone() {
                let mut close_menu = false;
                let menu_key: EdgeKey = (menu.info.source, menu.info.target, menu.info.kind);
                let label = codestory_graph::style::get_edge_kind_label(menu.info.kind);
                let edge_label = format!(
                    "{} {} {}",
                    menu.info.source_label, label, menu.info.target_label
                );

                egui::Area::new("edge_context_menu".into())
                    .fixed_pos(menu.pos)
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(&edge_label);
                            ui.separator();
                            if ui.button("Copy Edge").clicked() {
                                copy_text = Some(edge_label.clone());
                                close_menu = true;
                            }
                            if ui.button("Hide Edge").clicked() {
                                self.hidden_edge_keys.insert(menu_key);
                                self.rebuild_graph(settings);
                                close_menu = true;
                            }
                        });
                    });

                if close_menu {
                    self.edge_context_menu = None;
                }
            }

            if !ui.ctx().wants_keyboard_input() {
                let reset_zoom = ui.input(|i| i.key_pressed(egui::Key::Num0));
                if reset_zoom {
                    self.event_bus.publish(Event::ZoomReset);
                }

                if settings.show_legend && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.event_bus.publish(Event::SetShowLegend(false));
                }
            }

            if self.ui_trail_toolbar(ui, snarl_rect, settings) {
                view_state_dirty = true;
            }
            self.ui_zoom_cluster(ui, snarl_rect);
            self.ui_legend_button(ui, snarl_rect, settings);

            if settings.show_minimap {
                self.ui_minimap(ui, snarl_rect, settings);
            }

            if settings.show_legend {
                self.ui_legend(ui, snarl_rect, settings.show_minimap);
            }
        });

        if rebuild_needed {
            self.rebuild_graph(settings);
        }

        if let Some(text) = copy_text {
            ui.ctx().copy_text(text);
        }

        if let Some(image) = copy_image {
            ui.ctx().copy_image(image);
            show_info = Some("Graph image copied to clipboard.".to_string());
        }

        if let Some((path, line)) = scroll_to {
            self.event_bus.publish(Event::ScrollToLine {
                file: std::path::PathBuf::from(path),
                line: line as usize,
            });
        }

        if export_image {
            if let Some(image) = self.build_graph_image() {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Export Graph Image")
                    .set_file_name("codestory-graph.png")
                    .save_file()
                {
                    let path = ensure_png_extension(path);
                    match write_color_image_png(&path, &image) {
                        Ok(()) => {
                            show_info =
                                Some(format!("Exported graph image to {}.", path.display()));
                        }
                        Err(err) => {
                            show_info = Some(format!("Export failed: {}", err));
                        }
                    }
                }
            } else {
                show_info = Some("No graph data available to export.".to_string());
            }
        }

        if let Some(path) = open_folder {
            if !open_containing_folder(&path) {
                show_info = Some("Unable to open containing folder.".to_string());
            }
        }

        if let Some(path) = open_in_ide {
            if !open_in_default_app(&path) {
                show_info = Some("Unable to open file in IDE.".to_string());
            }
        }

        if let Some(message) = show_info {
            self.event_bus.publish(Event::ShowInfo { message });
        }

        NodeGraphViewResponse {
            clicked_node: self.clicked_node.or(node_to_navigate),
            view_state_dirty,
        }
    }

    fn build_positions(
        &self,
        settings: &crate::settings::NodeGraphSettings,
    ) -> HashMap<NodeId, egui::Pos2> {
        let mut positions = HashMap::new();
        if let Some(cached) = &self.cached_positions {
            positions.extend(cached.iter().map(|(k, v)| (*k, *v)));
        }
        for (id, pos) in &self.fallback_positions {
            positions.entry(*id).or_insert(*pos);
        }
        for (id, custom) in &settings.view_state.custom_positions {
            positions.insert(*id, egui::pos2(custom.x, custom.y));
        }
        positions
    }

    fn positions_bounds(&self, positions: &HashMap<NodeId, egui::Pos2>) -> Option<egui::Rect> {
        if self.uml_nodes.is_empty() {
            return None;
        }
        let mut min = egui::pos2(f32::INFINITY, f32::INFINITY);
        let mut max = egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
        let node_size = egui::vec2(160.0, 80.0);
        for node in &self.uml_nodes {
            let Some(pos) = positions.get(&node.id) else {
                continue;
            };
            min.x = min.x.min(pos.x);
            min.y = min.y.min(pos.y);
            max.x = max.x.max(pos.x + node_size.x);
            max.y = max.y.max(pos.y + node_size.y);
        }
        if !min.x.is_finite() || !min.y.is_finite() || !max.x.is_finite() || !max.y.is_finite() {
            None
        } else {
            Some(egui::Rect::from_min_max(min, max))
        }
    }

    fn node_display_name(&self, id: NodeId) -> Option<String> {
        self.node_lookup.get(&id).map(|node| {
            node.qualified_name
                .clone()
                .unwrap_or_else(|| node.serialized_name.clone())
        })
    }

    fn node_file_path(&self, id: NodeId) -> Option<String> {
        let node = self.node_lookup.get(&id)?;
        if node.kind == NodeKind::FILE {
            return Some(node.serialized_name.clone());
        }
        if let Some(file_id) = node.file_node_id {
            return self
                .node_lookup
                .get(&file_id)
                .map(|file_node| file_node.serialized_name.clone());
        }
        None
    }

    fn node_start_line(&self, id: NodeId) -> Option<u32> {
        self.node_lookup.get(&id).and_then(|node| node.start_line)
    }

    /// Renders the vertical toolbar on the left side of the graph viewport (Req 9.1-9.5).
    /// Returns true if a graph rebuild is needed.
    fn ui_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        settings: &crate::settings::NodeGraphSettings,
    ) -> bool {
        let mut rebuild_needed = false;
        let toolbar_width = 56.0;
        let palette = self.style_resolver.palette();
        let button_size = egui::vec2(36.0, 36.0);
        let is_light = is_light_color(palette.background);
        let toolbar_fill = adjust_color(
            palette.background,
            is_light,
            if is_light { 0.08 } else { 0.24 },
        );
        let toolbar_stroke = adjust_color(
            palette.node_section_border,
            is_light,
            if is_light { 0.30 } else { 0.45 },
        );

        ui.allocate_ui_with_layout(
            egui::vec2(toolbar_width, ui.available_height()),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.set_width(toolbar_width);
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);

                let frame = egui::Frame::NONE
                    .fill(toolbar_fill)
                    .inner_margin(6.0)
                    .corner_radius(egui::CornerRadius::same(6))
                    .stroke(egui::Stroke::new(1.0, toolbar_stroke));

                frame.show(ui, |ui| {
                    if rail_button(
                        ui,
                        button_size,
                        palette,
                        RailButtonContent::Text(ph::HOUSE),
                        false,
                    )
                    .on_hover_text("Reset graph layout")
                    .clicked()
                    {
                        rebuild_needed = true;
                    }

                    let mut show_minimap = settings.show_minimap;
                    if rail_button(
                        ui,
                        button_size,
                        palette,
                        RailButtonContent::Glyph(RailGlyph::Minimap),
                        show_minimap,
                    )
                    .on_hover_text("Toggle minimap")
                    .clicked()
                    {
                        show_minimap = !show_minimap;
                        self.event_bus.publish(Event::SetShowMinimap(show_minimap));
                    }

                    if rail_button(
                        ui,
                        button_size,
                        palette,
                        RailButtonContent::Text(ph::GEAR),
                        self.show_toolbar_panel,
                    )
                    .on_hover_text("Graph settings")
                    .clicked()
                    {
                        self.show_toolbar_panel = !self.show_toolbar_panel;
                    }

                    if self.is_calculating {
                        ui.add_space(4.0);
                        ui.spinner();
                    }
                });
            },
        );

        if self.show_toolbar_panel {
            egui::Window::new("Graph Settings")
                .collapsible(false)
                .resizable(false)
                .open(&mut self.show_toolbar_panel)
                .show(ui.ctx(), |ui| {
                    ui.label("Layout");
                    egui::ComboBox::from_id_salt("layout_selector")
                        .selected_text(format!("{:?}", settings.layout_algorithm))
                        .show_ui(ui, |ui| {
                            for (alg, label) in [
                                (
                                    codestory_events::LayoutAlgorithm::ForceDirected,
                                    "Force Directed",
                                ),
                                (codestory_events::LayoutAlgorithm::Radial, "Radial"),
                                (codestory_events::LayoutAlgorithm::Grid, "Grid"),
                                (
                                    codestory_events::LayoutAlgorithm::Hierarchical,
                                    "Hierarchical",
                                ),
                            ] {
                                if ui
                                    .selectable_label(settings.layout_algorithm == alg, label)
                                    .clicked()
                                {
                                    self.event_bus.publish(Event::SetLayoutMethod(alg));
                                }
                            }
                        });

                    if settings.layout_algorithm == codestory_events::LayoutAlgorithm::Hierarchical
                    {
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_value(
                                    &mut self.current_layout_direction,
                                    codestory_core::LayoutDirection::Horizontal,
                                    "H",
                                )
                                .changed()
                            {
                                self.event_bus.publish(Event::SetLayoutDirection(
                                    codestory_core::LayoutDirection::Horizontal,
                                ));
                                self.cached_positions = None;
                                rebuild_needed = true;
                            }
                            if ui
                                .selectable_value(
                                    &mut self.current_layout_direction,
                                    codestory_core::LayoutDirection::Vertical,
                                    "V",
                                )
                                .changed()
                            {
                                self.event_bus.publish(Event::SetLayoutDirection(
                                    codestory_core::LayoutDirection::Vertical,
                                ));
                                self.cached_positions = None;
                                rebuild_needed = true;
                            }
                        });
                    }

                    ui.separator();
                    ui.label("Filters");
                    let mut show_classes = settings.show_classes;
                    if ui.checkbox(&mut show_classes, "Classes").changed() {
                        self.event_bus.publish(Event::SetShowClasses(show_classes));
                    }
                    let mut show_functions = settings.show_functions;
                    if ui.checkbox(&mut show_functions, "Functions").changed() {
                        self.event_bus
                            .publish(Event::SetShowFunctions(show_functions));
                    }
                    let mut show_variables = settings.show_variables;
                    if ui.checkbox(&mut show_variables, "Variables").changed() {
                        self.event_bus
                            .publish(Event::SetShowVariables(show_variables));
                    }

                    ui.separator();
                    if ui.button("Expand All").clicked() {
                        self.event_bus.publish(Event::ExpandAll);
                    }
                    if ui.button("Collapse All").clicked() {
                        self.event_bus.publish(Event::CollapseAll);
                    }
                });
        }

        rebuild_needed
    }

    fn ui_trail_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        parent_rect: egui::Rect,
        settings: &mut crate::settings::NodeGraphSettings,
    ) -> bool {
        let palette = self.style_resolver.palette();
        let button_size = egui::vec2(26.0, 26.0);
        let cluster_pos = parent_rect.min + egui::vec2(10.0, 10.0);

        let is_light = is_light_color(palette.background);
        let panel_fill = adjust_color(
            palette.background,
            is_light,
            if is_light { 0.08 } else { 0.24 },
        );
        let panel_stroke = adjust_color(
            palette.node_section_border,
            is_light,
            if is_light { 0.30 } else { 0.45 },
        );

        let mut settings_changed = false;

        let publish_trail_config = |event_bus: &codestory_events::EventBus,
                                    depth: u32,
                                    direction: TrailDirection,
                                    edge_filter: Vec<EdgeKind>| {
            event_bus.publish(Event::TrailConfigChange {
                depth,
                direction,
                edge_filter,
            });
        };

        let matches_filter = |current: &[EdgeKind], preset: &[EdgeKind]| -> bool {
            if current.len() != preset.len() {
                return false;
            }
            let current_set: HashSet<EdgeKind> = current.iter().copied().collect();
            let preset_set: HashSet<EdgeKind> = preset.iter().copied().collect();
            current_set == preset_set
        };

        egui::Area::new("graph_trail_toolbar".into())
            .order(egui::Order::Foreground)
            .fixed_pos(cluster_pos)
            .show(ui.ctx(), |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);

                let frame = egui::Frame::NONE
                    .fill(panel_fill)
                    .inner_margin(6.0)
                    .corner_radius(egui::CornerRadius::same(6))
                    .stroke(egui::Stroke::new(1.0, panel_stroke));

                frame.show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        // Navigation / trail entry
                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Glyph(RailGlyph::TrailBack),
                            false,
                        )
                        .on_hover_text("Back")
                        .clicked()
                        {
                            self.event_bus.publish(Event::HistoryBack);
                        }

                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Glyph(RailGlyph::TrailForward),
                            false,
                        )
                        .on_hover_text("Forward")
                        .clicked()
                        {
                            self.event_bus.publish(Event::HistoryForward);
                        }

                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Glyph(RailGlyph::TrailCustom),
                            false,
                        )
                        .on_hover_text("Custom trail")
                        .clicked()
                        {
                            self.event_bus.publish(Event::OpenCustomTrailDialog);
                        }

                        ui.separator();

                        // Predefined edge filter presets (Sourcetrail-style)
                        let all_selected = settings.trail_edge_filter.is_empty();
                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text("All"),
                            all_selected,
                        )
                        .on_hover_text("Show all edge kinds")
                        .clicked()
                        {
                            settings.trail_edge_filter.clear();
                            settings_changed = true;
                            publish_trail_config(
                                &self.event_bus,
                                settings.trail_depth,
                                settings.trail_direction,
                                settings.trail_edge_filter.clone(),
                            );
                        }

                        let call_filter = [EdgeKind::CALL];
                        let call_selected =
                            matches_filter(&settings.trail_edge_filter, &call_filter);
                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text("Call"),
                            call_selected,
                        )
                        .on_hover_text("Call graph")
                        .clicked()
                        {
                            settings.trail_edge_filter = call_filter.to_vec();
                            settings_changed = true;
                            publish_trail_config(
                                &self.event_bus,
                                settings.trail_depth,
                                settings.trail_direction,
                                settings.trail_edge_filter.clone(),
                            );
                        }

                        let inheritance_filter = [EdgeKind::INHERITANCE, EdgeKind::OVERRIDE];
                        let inheritance_selected =
                            matches_filter(&settings.trail_edge_filter, &inheritance_filter);
                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text("Inh"),
                            inheritance_selected,
                        )
                        .on_hover_text("Inheritance / override")
                        .clicked()
                        {
                            settings.trail_edge_filter = inheritance_filter.to_vec();
                            settings_changed = true;
                            publish_trail_config(
                                &self.event_bus,
                                settings.trail_depth,
                                settings.trail_direction,
                                settings.trail_edge_filter.clone(),
                            );
                        }

                        let include_filter = [EdgeKind::INCLUDE, EdgeKind::IMPORT];
                        let include_selected =
                            matches_filter(&settings.trail_edge_filter, &include_filter);
                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text("Inc"),
                            include_selected,
                        )
                        .on_hover_text("Include / import tree")
                        .clicked()
                        {
                            settings.trail_edge_filter = include_filter.to_vec();
                            settings_changed = true;
                            publish_trail_config(
                                &self.event_bus,
                                settings.trail_depth,
                                settings.trail_direction,
                                settings.trail_edge_filter.clone(),
                            );
                        }

                        ui.separator();

                        // Trail direction (Incoming / Outgoing / Both)
                        let outgoing_selected =
                            settings.trail_direction == TrailDirection::Outgoing;
                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text(ph::ARROW_RIGHT),
                            outgoing_selected,
                        )
                        .on_hover_text("Outgoing")
                        .clicked()
                        {
                            settings.trail_direction = TrailDirection::Outgoing;
                            settings_changed = true;
                            publish_trail_config(
                                &self.event_bus,
                                settings.trail_depth,
                                settings.trail_direction,
                                settings.trail_edge_filter.clone(),
                            );
                        }

                        let both_selected = settings.trail_direction == TrailDirection::Both;
                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text(ph::ARROWS_LEFT_RIGHT),
                            both_selected,
                        )
                        .on_hover_text("Both directions")
                        .clicked()
                        {
                            settings.trail_direction = TrailDirection::Both;
                            settings_changed = true;
                            publish_trail_config(
                                &self.event_bus,
                                settings.trail_depth,
                                settings.trail_direction,
                                settings.trail_edge_filter.clone(),
                            );
                        }

                        let incoming_selected =
                            settings.trail_direction == TrailDirection::Incoming;
                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text(ph::ARROW_LEFT),
                            incoming_selected,
                        )
                        .on_hover_text("Incoming")
                        .clicked()
                        {
                            settings.trail_direction = TrailDirection::Incoming;
                            settings_changed = true;
                            publish_trail_config(
                                &self.event_bus,
                                settings.trail_depth,
                                settings.trail_direction,
                                settings.trail_edge_filter.clone(),
                            );
                        }

                        ui.separator();

                        // Depth: 1..=20 plus ∞ (stored as 0)
                        ui.label(
                            egui::RichText::new("Depth")
                                .size(11.0)
                                .color(palette.node_default_text),
                        );
                        let mut slider_pos: u32 = if settings.trail_depth == 0 {
                            20
                        } else {
                            (settings.trail_depth.saturating_sub(1)).min(19)
                        };
                        let slider = egui::Slider::new(&mut slider_pos, 0..=20)
                            .show_value(false)
                            .step_by(1.0);
                        let slider_response = ui.add_sized(egui::vec2(120.0, 18.0), slider);
                        if slider_response.changed() {
                            let new_depth = if slider_pos == 20 { 0 } else { slider_pos + 1 };
                            if new_depth != settings.trail_depth {
                                settings.trail_depth = new_depth;
                                settings_changed = true;
                                publish_trail_config(
                                    &self.event_bus,
                                    settings.trail_depth,
                                    settings.trail_direction,
                                    settings.trail_edge_filter.clone(),
                                );
                            }
                        }
                        let depth_label = if slider_pos == 20 {
                            "∞".to_string()
                        } else {
                            (slider_pos + 1).to_string()
                        };
                        ui.label(
                            egui::RichText::new(depth_label)
                                .size(11.0)
                                .color(palette.node_default_text),
                        );

                        ui.separator();

                        // Layout direction (graph layout orientation)
                        let horizontal_selected = settings.layout_direction
                            == codestory_core::LayoutDirection::Horizontal;
                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text("H"),
                            horizontal_selected,
                        )
                        .on_hover_text("Layout direction: Horizontal")
                        .clicked()
                        {
                            settings.layout_direction = codestory_core::LayoutDirection::Horizontal;
                            settings_changed = true;
                            self.event_bus.publish(Event::SetLayoutDirection(
                                codestory_core::LayoutDirection::Horizontal,
                            ));
                        }
                        let vertical_selected =
                            settings.layout_direction == codestory_core::LayoutDirection::Vertical;
                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text("V"),
                            vertical_selected,
                        )
                        .on_hover_text("Layout direction: Vertical")
                        .clicked()
                        {
                            settings.layout_direction = codestory_core::LayoutDirection::Vertical;
                            settings_changed = true;
                            self.event_bus.publish(Event::SetLayoutDirection(
                                codestory_core::LayoutDirection::Vertical,
                            ));
                        }

                        ui.separator();

                        // Grouping toggles
                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Glyph(RailGlyph::Namespace),
                            settings.group_by_namespace,
                        )
                        .on_hover_text("Group by namespace")
                        .clicked()
                        {
                            settings.group_by_namespace = !settings.group_by_namespace;
                            settings_changed = true;
                            self.event_bus
                                .publish(Event::SetGroupByNamespace(settings.group_by_namespace));
                        }

                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Glyph(RailGlyph::File),
                            settings.group_by_file,
                        )
                        .on_hover_text("Group by file")
                        .clicked()
                        {
                            settings.group_by_file = !settings.group_by_file;
                            settings_changed = true;
                            self.event_bus
                                .publish(Event::SetGroupByFile(settings.group_by_file));
                        }
                    });
                });
            });

        settings_changed
    }

    fn ui_zoom_cluster(&mut self, ui: &mut egui::Ui, parent_rect: egui::Rect) {
        let palette = self.style_resolver.palette();
        let button_size = egui::vec2(26.0, 26.0);
        let margin = 10.0;
        let inner_margin = 6.0;
        let cluster_pos = egui::pos2(
            parent_rect.min.x + margin,
            parent_rect.max.y - margin - (button_size.y + inner_margin * 2.0),
        );

        let is_light = is_light_color(palette.background);
        let panel_fill = adjust_color(
            palette.background,
            is_light,
            if is_light { 0.08 } else { 0.24 },
        );
        let panel_stroke = adjust_color(
            palette.node_section_border,
            is_light,
            if is_light { 0.30 } else { 0.45 },
        );

        egui::Area::new("graph_zoom_cluster".into())
            .order(egui::Order::Foreground)
            .fixed_pos(cluster_pos)
            .show(ui.ctx(), |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);

                let frame = egui::Frame::NONE
                    .fill(panel_fill)
                    .inner_margin(inner_margin)
                    .corner_radius(egui::CornerRadius::same(6))
                    .stroke(egui::Stroke::new(1.0, panel_stroke));

                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text(ph::MAGNIFYING_GLASS_MINUS),
                            false,
                        )
                        .on_hover_text("Zoom out")
                        .clicked()
                        {
                            self.event_bus.publish(Event::ZoomOut);
                        }

                        ui.label(
                            egui::RichText::new(format!("{:.0}%", self.current_zoom * 100.0))
                                .size(11.0)
                                .color(palette.node_default_text),
                        );

                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text(ph::MAGNIFYING_GLASS_PLUS),
                            false,
                        )
                        .on_hover_text("Zoom in")
                        .clicked()
                        {
                            self.event_bus.publish(Event::ZoomIn);
                        }

                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text("0"),
                            false,
                        )
                        .on_hover_text("Reset zoom (0)")
                        .clicked()
                        {
                            self.event_bus.publish(Event::ZoomReset);
                        }

                        if rail_button(
                            ui,
                            button_size,
                            palette,
                            RailButtonContent::Text(ph::ARROWS_OUT_SIMPLE),
                            false,
                        )
                        .on_hover_text("Zoom to fit")
                        .clicked()
                        {
                            self.event_bus.publish(Event::ZoomToFit);
                        }
                    });
                });
            });
    }

    fn ui_legend_button(
        &mut self,
        ui: &mut egui::Ui,
        parent_rect: egui::Rect,
        settings: &crate::settings::NodeGraphSettings,
    ) {
        let palette = self.style_resolver.palette();
        let button_size = egui::vec2(26.0, 26.0);
        let margin = 10.0;
        let inner_margin = 6.0;
        let reserve_y = if settings.show_minimap {
            // Minimap is 100px tall and already has a 10px bottom margin; reserve space above it.
            100.0 + margin
        } else {
            0.0
        };
        let button_pos = egui::pos2(
            parent_rect.max.x - margin - (button_size.x + inner_margin * 2.0),
            parent_rect.max.y - margin - reserve_y - (button_size.y + inner_margin * 2.0),
        );

        let is_light = is_light_color(palette.background);
        let panel_fill = adjust_color(
            palette.background,
            is_light,
            if is_light { 0.08 } else { 0.24 },
        );
        let panel_stroke = adjust_color(
            palette.node_section_border,
            is_light,
            if is_light { 0.30 } else { 0.45 },
        );

        egui::Area::new("graph_legend_button".into())
            .order(egui::Order::Foreground)
            .fixed_pos(button_pos)
            .show(ui.ctx(), |ui| {
                let frame = egui::Frame::NONE
                    .fill(panel_fill)
                    .inner_margin(inner_margin)
                    .corner_radius(egui::CornerRadius::same(6))
                    .stroke(egui::Stroke::new(1.0, panel_stroke));

                frame.show(ui, |ui| {
                    if rail_button(
                        ui,
                        button_size,
                        palette,
                        RailButtonContent::Text(ph::QUESTION),
                        settings.show_legend,
                    )
                    .on_hover_text("Toggle legend")
                    .clicked()
                    {
                        self.event_bus
                            .publish(Event::SetShowLegend(!settings.show_legend));
                    }
                });
            });
    }

    fn ui_minimap(
        &mut self,
        ui: &mut egui::Ui,
        parent_rect: egui::Rect,
        settings: &crate::settings::NodeGraphSettings,
    ) {
        let palette = self.style_resolver.palette();
        let minimap_size = egui::vec2(150.0, 100.0);
        let minimap_rect = egui::Rect::from_min_size(
            parent_rect.right_bottom() - minimap_size - egui::vec2(10.0, 10.0),
            minimap_size,
        );

        // Render within the graph layer so it stays above the graph but below global UI.
        let painter = ui.painter_at(parent_rect);

        painter.rect_filled(minimap_rect, 5.0, palette.minimap_background);
        painter.rect_stroke(
            minimap_rect,
            5.0,
            (1.0, palette.minimap_border),
            egui::StrokeKind::Middle,
        );
        let positions = self.build_positions(settings);
        let Some(bounds) = self.positions_bounds(&positions) else {
            return;
        };

        let scale = (minimap_size.x / bounds.width())
            .min(minimap_size.y / bounds.height())
            .min(1.0);
        let offset = minimap_rect.center() - bounds.center() * scale;

        for node in &self.uml_nodes {
            let Some(pos) = positions.get(&node.id) else {
                continue;
            };
            let map_pos = *pos * scale + offset;
            let size = egui::vec2(100.0, 40.0) * scale;
            painter.rect_filled(
                egui::Rect::from_min_size(map_pos, size),
                1.0,
                palette.minimap_node,
            );
        }
    }

    fn build_graph_image(&self) -> Option<egui::ColorImage> {
        if self.last_node_rects_graph.is_empty() {
            return None;
        }

        let mut screen_rects: HashMap<NodeId, egui::Rect> = HashMap::new();
        let mut bounds: Option<egui::Rect> = None;
        for (node_id, rect) in &self.last_node_rects_graph {
            let screen_rect = self.current_transform.mul_rect(*rect);
            screen_rects.insert(*node_id, screen_rect);
            bounds = Some(if let Some(existing) = bounds {
                existing.union(screen_rect)
            } else {
                screen_rect
            });
        }
        let bounds = bounds?;

        let palette = self.style_resolver.palette();
        let padding = 40.0;
        let max_dim = 4000.0;
        let hard_max_dim = 12000.0;
        let width = bounds.width().max(1.0);
        let height = bounds.height().max(1.0);
        let export_font = load_export_font();
        let zoom = self.current_zoom.max(0.1);

        let base_header_font = 13.0 * zoom;
        let mut scale = (max_dim / width).min(max_dim / height).max(0.1);
        if base_header_font > 0.0 {
            let min_font_px = 10.0;
            let min_scale = min_font_px / base_header_font;
            let max_scale = (hard_max_dim / width).min(hard_max_dim / height).max(0.1);
            scale = scale.max(min_scale).min(max_scale);
        }

        let img_width = ((width * scale) + padding * 2.0).ceil().max(1.0) as usize;
        let img_height = ((height * scale) + padding * 2.0).ceil().max(1.0) as usize;
        let mut image = egui::ColorImage::filled([img_width, img_height], palette.background);

        let origin = bounds.min;
        let map_pos = |pos: egui::Pos2| -> (i32, i32) {
            let x = ((pos.x - origin.x) * scale + padding).round() as i32;
            let y = ((pos.y - origin.y) * scale + padding).round() as i32;
            (x, y)
        };
        let map_pos_f = |pos: egui::Pos2| -> (f32, f32) {
            let x = (pos.x - origin.x) * scale + padding;
            let y = (pos.y - origin.y) * scale + padding;
            (x, y)
        };
        let map_rect = |rect: egui::Rect| -> (i32, i32, i32, i32) {
            let (x0, y0) = map_pos(rect.min);
            let (x1, y1) = map_pos(rect.max);
            let min_x = x0.min(x1);
            let mut max_x = x0.max(x1);
            let min_y = y0.min(y1);
            let mut max_y = y0.max(y1);
            if max_x == min_x {
                max_x += 1;
            }
            if max_y == min_y {
                max_y += 1;
            }
            (min_x, min_y, max_x, max_y)
        };

        let thickness = ((2.0 * scale).round() as i32).max(1);
        let uml_lookup: std::collections::HashMap<NodeId, &UmlNode> =
            self.uml_nodes.iter().map(|uml| (uml.id, uml)).collect();

        for edge in &self.current_edges {
            let (Some(src_rect), Some(dst_rect)) = (
                screen_rects.get(&edge.source),
                screen_rects.get(&edge.target),
            ) else {
                continue;
            };
            let source_rect = to_uml_rect(*src_rect);
            let target_rect = to_uml_rect(*dst_rect);
            let curve = self
                .edge_overlay
                .router
                .route_edge(source_rect, target_rect);
            let color = self.style_resolver.resolve_edge_color(edge.kind);
            let dist = ((curve.end.x - curve.start.x).powi(2)
                + (curve.end.y - curve.start.y).powi(2))
            .sqrt();
            let samples = ((dist / 40.0).ceil() as usize).clamp(12, 64);
            let mut prev: Option<(i32, i32)> = None;
            for i in 0..=samples {
                let t = i as f32 / samples as f32;
                let p = curve.sample(t);
                let (x, y) = map_pos_f(egui::pos2(p.x, p.y));
                let x = x.round() as i32;
                let y = y.round() as i32;
                if let Some((px, py)) = prev {
                    draw_line(&mut image, px, py, x, y, color, thickness);
                }
                prev = Some((x, y));
            }
        }

        for (node_id, rect) in &screen_rects {
            let Some(uml) = uml_lookup.get(node_id) else {
                continue;
            };
            let collapse_state = self.last_view_state.get_collapse_state(*node_id);
            let layout = export_layout_for_node(uml, *rect, &collapse_state, zoom);

            let (min_x, min_y, max_x, max_y) = map_rect(*rect);
            draw_rect_filled(
                &mut image,
                min_x,
                min_y,
                max_x,
                max_y,
                palette.node_default_fill,
            );
            draw_rect_stroke(
                &mut image,
                min_x,
                min_y,
                max_x,
                max_y,
                palette.node_default_border,
                thickness.max(1),
            );

            let header_color = self.style_resolver.resolve_node_color(uml.kind);
            let header_text_color = self.style_resolver.resolve_text_color(header_color);
            let (hx0, hy0, hx1, hy1) = map_rect(layout.header_rect);
            draw_rect_filled(&mut image, hx0, hy0, hx1, hy1, header_color);

            let header_label = export_header_label(uml);
            let header_font = 13.0 * zoom * scale;
            if export_font.is_some() {
                let header_x = layout.header_rect.min.x + 10.0 * zoom;
                let header_y = layout.header_rect.center().y;
                let (text_x, center_y_px) = map_pos_f(egui::pos2(header_x, header_y));
                let max_header_width = (layout.header_rect.width() - 20.0 * zoom).max(10.0) * scale;
                let header_font = fit_text_size(
                    &export_font,
                    &header_label,
                    header_font,
                    max_header_width,
                    6.0,
                );
                draw_text(
                    &mut image,
                    &export_font,
                    &header_label,
                    header_font,
                    text_x,
                    center_y_px,
                    header_text_color,
                );

                if collapse_state.is_collapsed {
                    let total_members: usize = uml
                        .visibility_sections
                        .iter()
                        .map(|section| section.members.len())
                        .sum();
                    if total_members > 0 {
                        let badge = format!("[{}]", total_members);
                        let badge_width =
                            measure_text_width(&export_font, &badge, 10.0 * zoom * scale)
                                .unwrap_or(0.0);
                        let badge_x = layout.header_rect.max.x - 10.0 * zoom;
                        let badge_y = layout.header_rect.center().y;
                        let (badge_right_px, badge_center_px) =
                            map_pos_f(egui::pos2(badge_x, badge_y));
                        let bx = badge_right_px - badge_width;
                        draw_text(
                            &mut image,
                            &export_font,
                            &badge,
                            10.0 * zoom * scale,
                            bx,
                            badge_center_px,
                            header_text_color,
                        );
                    }
                }
            }

            if collapse_state.is_collapsed {
                continue;
            }

            let section_font = 9.5 * zoom * scale;
            let member_font = 10.5 * zoom * scale;

            for section in &layout.sections {
                let (sx0, sy0, sx1, sy1) = map_rect(section.header_rect);
                draw_rect_filled(&mut image, sx0, sy0, sx1, sy1, palette.node_section_fill);
                draw_rect_stroke(
                    &mut image,
                    sx0,
                    sy0,
                    sx1,
                    sy1,
                    palette.node_section_border,
                    thickness.max(1),
                );

                let label = section.kind.label();
                let label_width =
                    measure_text_width(&export_font, label, section_font).unwrap_or(0.0);
                let chip_padding = egui::vec2(6.0 * zoom, 2.0 * zoom);
                let chip_size = egui::vec2(
                    label_width / scale + chip_padding.x * 2.0,
                    section.header_rect.height() - 4.0 * zoom,
                );
                let chip_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        section.header_rect.min.x + 4.0 * zoom,
                        section.header_rect.min.y + 2.0 * zoom,
                    ),
                    chip_size,
                );
                let (cx0, cy0, cx1, cy1) = map_rect(chip_rect);
                draw_pill(&mut image, cx0, cy0, cx1, cy1, palette.section_label_fill);

                let mut text_offset_x = chip_padding.x;
                if let Some(icon_kind) = export_section_icon_kind(section.kind) {
                    let icon_size = 8.0 * zoom;
                    let icon_center = egui::pos2(
                        chip_rect.min.x + chip_padding.x + icon_size * 0.5,
                        chip_rect.center().y,
                    );
                    let (ix, iy) = map_pos_f(icon_center);
                    let icon_radius = (icon_size * 0.5 * scale).max(1.0);
                    let icon_color = self.style_resolver.resolve_icon_color(uml.kind);
                    match icon_kind {
                        ExportSectionIcon::Public => {
                            draw_circle_stroke(
                                &mut image,
                                ix,
                                iy,
                                icon_radius,
                                (1.2 * zoom * scale).max(1.0),
                                icon_color,
                            );
                        }
                        ExportSectionIcon::Private => {
                            let size = (icon_size * 0.8) * scale;
                            let min = egui::pos2(ix - size * 0.5, iy - size * 0.5);
                            let max = egui::pos2(ix + size * 0.5, iy + size * 0.5);
                            let (px0, py0) = (min.x.round() as i32, min.y.round() as i32);
                            let (px1, py1) = (max.x.round() as i32, max.y.round() as i32);
                            draw_rect_filled(&mut image, px0, py0, px1, py1, icon_color);
                        }
                    }
                    text_offset_x += icon_size + 4.0 * zoom;
                }

                let text_pos = egui::pos2(chip_rect.min.x + text_offset_x, chip_rect.center().y);
                let (tx, center_py) = map_pos_f(text_pos);
                draw_text(
                    &mut image,
                    &export_font,
                    label,
                    section_font,
                    tx,
                    center_py,
                    palette.section_label_text,
                );

                for (member, member_rect) in &section.members {
                    let member_label = export_member_label(member);
                    let label_width =
                        measure_text_width(&export_font, &member_label, member_font).unwrap_or(0.0);
                    let pill_padding = egui::vec2(6.0 * zoom, 3.0 * zoom);
                    let pill_height = member_rect.height() - 6.0 * zoom;
                    let pill_width = label_width / scale + pill_padding.x * 2.0;
                    let pill_rect = egui::Rect::from_min_size(
                        egui::pos2(
                            member_rect.min.x + 4.0 * zoom,
                            member_rect.center().y - pill_height * 0.5,
                        ),
                        egui::vec2(pill_width, pill_height),
                    );
                    let (px0, py0, px1, py1) = map_rect(pill_rect);
                    let pill_color = self.style_resolver.resolve_node_color(member.kind);
                    let pill_text = self.style_resolver.resolve_text_color(pill_color);
                    draw_pill(&mut image, px0, py0, px1, py1, pill_color);

                    let (mx, center_py) = map_pos_f(egui::pos2(
                        pill_rect.min.x + pill_padding.x,
                        pill_rect.center().y,
                    ));
                    draw_text(
                        &mut image,
                        &export_font,
                        &member_label,
                        member_font,
                        mx,
                        center_py,
                        pill_text,
                    );

                    if member.has_outgoing_edges {
                        let arrow_center = egui::pos2(
                            member_rect.max.x - 12.0 * zoom,
                            member_rect.min.y + 8.0 * zoom,
                        );
                        let (ax, ay) = map_pos_f(arrow_center);
                        draw_arrow_right(
                            &mut image,
                            ax,
                            ay,
                            6.0 * zoom * scale,
                            self.style_resolver.resolve_outgoing_edge_indicator_color(),
                            (1.2 * zoom * scale).max(1.0),
                        );
                    }
                }
            }
        }

        Some(image)
    }

    fn ui_legend(&self, ui: &mut egui::Ui, parent_rect: egui::Rect, minimap_enabled: bool) {
        let palette = self.style_resolver.palette();
        let legend_size = egui::vec2(150.0, 230.0);
        let margin = 10.0;
        let reserve_y = if minimap_enabled {
            // Minimap is 100px tall and already has a 10px bottom margin; reserve space above it.
            100.0 + margin
        } else {
            0.0
        };
        let legend_rect = egui::Rect::from_min_size(
            egui::pos2(
                parent_rect.max.x - margin - legend_size.x,
                parent_rect.max.y - margin - reserve_y - legend_size.y,
            ),
            legend_size,
        );

        egui::Window::new("Legend")
            .fixed_pos(legend_rect.min)
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .frame(egui::Frame::window(ui.style()).fill(palette.legend_background))
            .show(ui.ctx(), |ui| {
                ui.label(egui::RichText::new("Legend").strong());
                ui.separator();

                let node_items = [
                    ("Class", palette.type_fill),
                    ("Struct", palette.type_fill),
                    ("Function", palette.function_fill),
                    ("Module", palette.namespace_fill),
                    ("Variable", palette.variable_fill),
                ];

                for (name, color) in node_items {
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_at_least(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, color);
                        ui.label(name);
                    });
                }

                ui.add_space(6.0);
                ui.label(egui::RichText::new("Edges").strong());
                ui.separator();

                let edge_items = [
                    (
                        "Call",
                        self.style_resolver.resolve_edge_color(EdgeKind::CALL),
                    ),
                    (
                        "Use",
                        self.style_resolver.resolve_edge_color(EdgeKind::USAGE),
                    ),
                    (
                        "Override",
                        self.style_resolver.resolve_edge_color(EdgeKind::OVERRIDE),
                    ),
                    (
                        "Type",
                        self.style_resolver.resolve_edge_color(EdgeKind::TYPE_USAGE),
                    ),
                    (
                        "Include",
                        self.style_resolver.resolve_edge_color(EdgeKind::INCLUDE),
                    ),
                    ("Bundled", self.style_resolver.resolve_bundled_edge_color()),
                ];

                for (name, color) in edge_items {
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_at_least(egui::vec2(18.0, 10.0), egui::Sense::hover());
                        let mid = rect.center().y;
                        ui.painter().line_segment(
                            [egui::pos2(rect.left(), mid), egui::pos2(rect.right(), mid)],
                            egui::Stroke::new(1.6, color),
                        );
                        ui.label(name);
                    });
                }
            });
    }
}

struct ExportSectionLayout<'a> {
    kind: VisibilityKind,
    header_rect: egui::Rect,
    members: Vec<(&'a MemberItem, egui::Rect)>,
}

struct ExportNodeLayout<'a> {
    header_rect: egui::Rect,
    sections: Vec<ExportSectionLayout<'a>>,
}

#[derive(Clone, Copy)]
enum RailButtonContent<'a> {
    Text(&'a str),
    Glyph(RailGlyph),
}

#[derive(Clone, Copy)]
enum RailGlyph {
    Namespace,
    File,
    Minimap,
    TrailBack,
    TrailForward,
    TrailCustom,
}

fn rail_button(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    palette: crate::components::node_graph::style_resolver::GraphPalette,
    content: RailButtonContent<'_>,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let is_light = is_light_color(palette.background);
    let mut fill = palette.node_default_fill;
    if selected {
        fill = adjust_color(fill, is_light, 0.18);
    }
    if response.hovered() {
        fill = adjust_color(fill, is_light, 0.12);
    }
    if response.is_pointer_button_down_on() {
        fill = adjust_color(fill, is_light, 0.28);
    }
    let stroke = if selected {
        palette.node_default_border
    } else {
        palette.node_section_border
    };

    ui.painter().rect_filled(rect, 5.0, fill);
    ui.painter().rect_stroke(
        rect,
        5.0,
        egui::Stroke::new(1.0, stroke),
        egui::StrokeKind::Middle,
    );
    match content {
        RailButtonContent::Text(label) => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(12.0),
                palette.node_default_text,
            );
        }
        RailButtonContent::Glyph(glyph) => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                glyph_label(glyph),
                egui::FontId::proportional(12.0),
                palette.node_default_text,
            );
        }
    }

    response
}

fn glyph_label(glyph: RailGlyph) -> &'static str {
    match glyph {
        RailGlyph::Namespace => ph::BRACKETS_CURLY,
        RailGlyph::File => ph::FILE,
        RailGlyph::Minimap => ph::MAP_TRIFOLD,
        RailGlyph::TrailBack => ph::ARROW_LEFT,
        RailGlyph::TrailForward => ph::ARROW_RIGHT,
        RailGlyph::TrailCustom => ph::PATH,
    }
}

fn adjust_color(color: egui::Color32, is_light: bool, amount: f32) -> egui::Color32 {
    let target = if is_light {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    };
    mix_color(color, target, amount)
}

fn mix_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let (ar, ag, ab, aa) = a.to_tuple();
    let (br, bg, bb, ba) = b.to_tuple();
    let lerp = |x: u8, y: u8| -> u8 {
        ((x as f32 * (1.0 - t) + y as f32 * t).round() as i32).clamp(0, 255) as u8
    };
    egui::Color32::from_rgba_unmultiplied(lerp(ar, br), lerp(ag, bg), lerp(ab, bb), lerp(aa, ba))
}

fn to_uml_rect(rect: egui::Rect) -> codestory_graph::uml_types::Rect {
    codestory_graph::uml_types::Rect {
        min: codestory_graph::Vec2 {
            x: rect.min.x,
            y: rect.min.y,
        },
        max: codestory_graph::Vec2 {
            x: rect.max.x,
            y: rect.max.y,
        },
    }
}

fn is_light_color(color: egui::Color32) -> bool {
    let (r, g, b, _) = color.to_tuple();
    let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    luminance > 140.0
}

#[derive(Clone, Copy)]
enum ExportSectionIcon {
    Public,
    Private,
}

fn export_section_icon_kind(kind: VisibilityKind) -> Option<ExportSectionIcon> {
    match kind {
        VisibilityKind::Public => Some(ExportSectionIcon::Public),
        VisibilityKind::Private => Some(ExportSectionIcon::Private),
        _ => None,
    }
}

fn export_member_label(member: &MemberItem) -> String {
    if let Some(signature) = &member.signature {
        format!("{}{}", member.name, signature)
    } else {
        member.name.clone()
    }
}

fn export_header_label(node: &UmlNode) -> String {
    if let Some(bundle) = &node.bundle_info {
        format!("{} ({})", node.label, bundle.count)
    } else {
        node.label.clone()
    }
}

fn export_layout_for_node<'a>(
    node: &'a UmlNode,
    rect: egui::Rect,
    collapse_state: &CollapseState,
    zoom: f32,
) -> ExportNodeLayout<'a> {
    let header_height = 32.0 * zoom;
    let section_header_height = 20.0 * zoom;
    let member_row_height = 22.0 * zoom;
    let padding = 12.0 * zoom;

    let header_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), header_height));
    let mut sections = Vec::new();

    let mut cursor_y = header_rect.max.y + padding * 0.2;
    if !collapse_state.is_collapsed {
        for section in &node.visibility_sections {
            if section.members.is_empty() {
                continue;
            }
            let section_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + padding * 0.5, cursor_y),
                egui::vec2(rect.width() - padding, section_header_height),
            );
            cursor_y += section_header_height;

            let mut members = Vec::new();
            if !collapse_state.is_section_collapsed(section.kind) {
                for member in &section.members {
                    let member_rect = egui::Rect::from_min_size(
                        egui::pos2(rect.min.x + padding * 0.6, cursor_y),
                        egui::vec2(rect.width() - padding, member_row_height),
                    );
                    members.push((member, member_rect));
                    cursor_y += member_row_height;
                }
            }
            cursor_y += padding * 0.2;
            sections.push(ExportSectionLayout {
                kind: section.kind,
                header_rect: section_rect,
                members,
            });
        }
    }

    ExportNodeLayout {
        header_rect,
        sections,
    }
}

fn load_export_font() -> Option<fontdue::Font> {
    let font_dir = find_sourcetrail_fonts_dir()?;
    let path = font_dir.join("SourceCodePro-Regular.otf");
    let bytes = std::fs::read(path).ok()?;
    fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
}

fn find_sourcetrail_fonts_dir() -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("CODESTORY_FONT_DIR") {
        let path = std::path::PathBuf::from(dir);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(dir) = std::env::var("CODESTORY_SOURCETRAIL_DIR") {
        let path = std::path::PathBuf::from(dir)
            .join("bin")
            .join("app")
            .join("data")
            .join("fonts");
        if path.exists() {
            return Some(path);
        }
    }

    let mut current = std::env::current_dir().ok()?;
    for _ in 0..6 {
        let candidate = current
            .join("Sourcetrail")
            .join("bin")
            .join("app")
            .join("data")
            .join("fonts");
        if candidate.exists() {
            return Some(candidate);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn measure_text_width(font: &Option<fontdue::Font>, text: &str, size: f32) -> Option<f32> {
    let font = font.as_ref()?;
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    let mut settings = LayoutSettings::default();
    settings.x = 0.0;
    settings.y = 0.0;
    layout.reset(&settings);
    layout.append(&[font], &TextStyle::new(text, size, 0));
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    for glyph in layout.glyphs() {
        min_x = min_x.min(glyph.x);
        max_x = max_x.max(glyph.x + glyph.width as f32);
    }
    if min_x.is_finite() && max_x.is_finite() {
        Some((max_x - min_x).max(0.0))
    } else {
        Some(0.0)
    }
}

fn fit_text_size(
    font: &Option<fontdue::Font>,
    text: &str,
    size: f32,
    max_width: f32,
    min_size: f32,
) -> f32 {
    if max_width <= 0.0 {
        return size;
    }
    let width = measure_text_width(font, text, size).unwrap_or(0.0);
    if width <= max_width || width <= 0.0 {
        return size;
    }
    let scaled = size * (max_width / width);
    scaled.max(min_size)
}

fn draw_text(
    image: &mut egui::ColorImage,
    font: &Option<fontdue::Font>,
    text: &str,
    size: f32,
    x: f32,
    center_y: f32,
    color: egui::Color32,
) {
    let Some(font) = font.as_ref() else {
        return;
    };
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    let mut settings = LayoutSettings::default();
    if let Some(metrics) = font.horizontal_line_metrics(size) {
        let baseline = center_y + (metrics.ascent + metrics.descent) * 0.5;
        settings.y = baseline - metrics.ascent;
    } else {
        settings.y = center_y - size * 0.5;
    }
    settings.x = x;
    layout.reset(&settings);
    layout.append(&[font], &TextStyle::new(text, size, 0));

    for glyph in layout.glyphs() {
        if glyph.width == 0 || glyph.height == 0 {
            continue;
        }
        let (metrics, bitmap) = font.rasterize_indexed(glyph.key.glyph_index, glyph.key.px);
        blend_glyph(
            image,
            glyph.x,
            glyph.y,
            metrics.width,
            metrics.height,
            &bitmap,
            color,
        );
    }
}

fn blend_glyph(
    image: &mut egui::ColorImage,
    x: f32,
    y: f32,
    width: usize,
    height: usize,
    bitmap: &[u8],
    color: egui::Color32,
) {
    let start_x = x.floor() as i32;
    let start_y = y.floor() as i32;
    for row in 0..height {
        for col in 0..width {
            let alpha = bitmap[row * width + col];
            if alpha == 0 {
                continue;
            }
            let px = start_x + col as i32;
            let py = start_y + row as i32;
            blend_pixel(image, px, py, color, alpha);
        }
    }
}

fn draw_pill(
    image: &mut egui::ColorImage,
    min_x: i32,
    min_y: i32,
    max_x: i32,
    max_y: i32,
    color: egui::Color32,
) {
    let width = (max_x - min_x).max(1);
    let height = (max_y - min_y).max(1);
    let radius = (height / 2).min(width / 2).max(1);
    let mid_left = min_x + radius;
    let mid_right = max_x - radius;
    draw_rect_filled(image, mid_left, min_y, mid_right, max_y, color);
    draw_circle_filled(
        image,
        mid_left as f32,
        (min_y + max_y) as f32 * 0.5,
        radius as f32,
        color,
    );
    draw_circle_filled(
        image,
        mid_right as f32,
        (min_y + max_y) as f32 * 0.5,
        radius as f32,
        color,
    );
}

fn draw_circle_filled(
    image: &mut egui::ColorImage,
    cx: f32,
    cy: f32,
    radius: f32,
    color: egui::Color32,
) {
    let r = radius.max(1.0);
    let r_sq = r * r;
    let min_x = (cx - r).floor() as i32;
    let max_x = (cx + r).ceil() as i32;
    let min_y = (cy - r).floor() as i32;
    let max_y = (cy + r).ceil() as i32;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= r_sq {
                set_pixel(image, x, y, color);
            }
        }
    }
}

fn draw_circle_stroke(
    image: &mut egui::ColorImage,
    cx: f32,
    cy: f32,
    radius: f32,
    thickness: f32,
    color: egui::Color32,
) {
    let r = radius.max(1.0);
    let t = thickness.max(1.0);
    let outer = r;
    let inner = (r - t).max(0.0);
    let outer_sq = outer * outer;
    let inner_sq = inner * inner;
    let min_x = (cx - outer).floor() as i32;
    let max_x = (cx + outer).ceil() as i32;
    let min_y = (cy - outer).floor() as i32;
    let max_y = (cy + outer).ceil() as i32;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist_sq = dx * dx + dy * dy;
            if dist_sq <= outer_sq && dist_sq >= inner_sq {
                set_pixel(image, x, y, color);
            }
        }
    }
}

fn draw_arrow_right(
    image: &mut egui::ColorImage,
    cx: f32,
    cy: f32,
    size: f32,
    color: egui::Color32,
    thickness: f32,
) {
    let half = size * 0.5;
    let line_thickness = thickness.max(1.0).round() as i32;
    draw_line(
        image,
        (cx - half).round() as i32,
        cy.round() as i32,
        (cx + half).round() as i32,
        cy.round() as i32,
        color,
        line_thickness,
    );
    draw_line(
        image,
        (cx + half).round() as i32,
        cy.round() as i32,
        (cx + half - size * 0.35).round() as i32,
        (cy - size * 0.35).round() as i32,
        color,
        line_thickness,
    );
    draw_line(
        image,
        (cx + half).round() as i32,
        cy.round() as i32,
        (cx + half - size * 0.35).round() as i32,
        (cy + size * 0.35).round() as i32,
        color,
        line_thickness,
    );
}

fn blend_pixel(image: &mut egui::ColorImage, x: i32, y: i32, color: egui::Color32, alpha: u8) {
    let width = image.size[0] as i32;
    let height = image.size[1] as i32;
    if x < 0 || y < 0 || x >= width || y >= height {
        return;
    }
    let idx = (y * width + x) as usize;
    if let Some(pixel) = image.pixels.get_mut(idx) {
        let (r, g, b, a) = pixel.to_tuple();
        let alpha_f = alpha as f32 / 255.0;
        let src_a = (color.a() as f32 / 255.0) * alpha_f;
        let inv = 1.0 - src_a;
        let out_r = (color.r() as f32 * src_a + r as f32 * inv).round() as u8;
        let out_g = (color.g() as f32 * src_a + g as f32 * inv).round() as u8;
        let out_b = (color.b() as f32 * src_a + b as f32 * inv).round() as u8;
        let out_a = ((color.a() as f32 * src_a + a as f32 * inv).round() as u8).max(a);
        *pixel = egui::Color32::from_rgba_unmultiplied(out_r, out_g, out_b, out_a);
    }
}

fn open_containing_folder(path: &str) -> bool {
    let path = std::path::Path::new(path);

    #[cfg(target_os = "windows")]
    {
        let target = path.to_string_lossy().to_string();
        return std::process::Command::new("explorer")
            .arg("/select,")
            .arg(target)
            .spawn()
            .is_ok();
    }

    #[cfg(target_os = "macos")]
    {
        let Some(parent) = path.parent() else {
            return false;
        };
        return std::process::Command::new("open")
            .arg(parent)
            .spawn()
            .is_ok();
    }

    #[cfg(target_os = "linux")]
    {
        let Some(parent) = path.parent() else {
            return false;
        };
        return std::process::Command::new("xdg-open")
            .arg(parent)
            .spawn()
            .is_ok();
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        false
    }
}

fn open_in_default_app(path: &str) -> bool {
    let path = std::path::Path::new(path);

    #[cfg(target_os = "windows")]
    {
        let target = path.to_string_lossy().to_string();
        return std::process::Command::new("cmd")
            .args(["/C", "start", "", &target])
            .spawn()
            .is_ok();
    }

    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("open").arg(path).spawn().is_ok();
    }

    #[cfg(target_os = "linux")]
    {
        return std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .is_ok();
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        false
    }
}

fn ensure_png_extension(path: std::path::PathBuf) -> std::path::PathBuf {
    if path.extension().is_some() {
        path
    } else {
        path.with_extension("png")
    }
}

fn write_color_image_png(path: &std::path::Path, image: &egui::ColorImage) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        let (r, g, b, a) = pixel.to_tuple();
        bytes.extend_from_slice(&[r, g, b, a]);
    }
    image::save_buffer(
        path,
        &bytes,
        image.size[0] as u32,
        image.size[1] as u32,
        image::ColorType::Rgba8,
    )
    .map_err(|err| err.to_string())
}

fn draw_rect_filled(
    image: &mut egui::ColorImage,
    min_x: i32,
    min_y: i32,
    max_x: i32,
    max_y: i32,
    color: egui::Color32,
) {
    for y in min_y..max_y {
        for x in min_x..max_x {
            set_pixel(image, x, y, color);
        }
    }
}

fn draw_rect_stroke(
    image: &mut egui::ColorImage,
    min_x: i32,
    min_y: i32,
    max_x: i32,
    max_y: i32,
    color: egui::Color32,
    thickness: i32,
) {
    let thickness = thickness.max(1);
    for offset in 0..thickness {
        let y_top = min_y + offset;
        let y_bottom = max_y - 1 - offset;
        for x in min_x..max_x {
            set_pixel(image, x, y_top, color);
            set_pixel(image, x, y_bottom, color);
        }
        let x_left = min_x + offset;
        let x_right = max_x - 1 - offset;
        for y in min_y..max_y {
            set_pixel(image, x_left, y, color);
            set_pixel(image, x_right, y, color);
        }
    }
}

fn draw_line(
    image: &mut egui::ColorImage,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    color: egui::Color32,
    thickness: i32,
) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        draw_thick_point(image, x0, y0, color, thickness);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn draw_thick_point(
    image: &mut egui::ColorImage,
    x: i32,
    y: i32,
    color: egui::Color32,
    thickness: i32,
) {
    let radius = (thickness.max(1) - 1) / 2;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            set_pixel(image, x + dx, y + dy, color);
        }
    }
}

fn set_pixel(image: &mut egui::ColorImage, x: i32, y: i32, color: egui::Color32) {
    let width = image.size[0] as i32;
    let height = image.size[1] as i32;
    if x < 0 || y < 0 || x >= width || y >= height {
        return;
    }
    let idx = (y * width + x) as usize;
    if let Some(pixel) = image.pixels.get_mut(idx) {
        *pixel = color;
    }
}
