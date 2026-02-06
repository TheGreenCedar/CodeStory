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
    u64,
    Option<NodeId>,
    Vec<Node>,
    Vec<Edge>,
    codestory_events::LayoutAlgorithm,
    codestory_core::LayoutDirection,
);

type LayoutResponse = (u64, bool, HashMap<NodeId, egui::Pos2>);

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
    graph_context_menu_pos: Option<egui::Pos2>,
    last_node_rects_graph: HashMap<NodeId, egui::Rect>,
    last_view_state: GraphViewState,

    // Data Cache for rebuilding
    cached_data: Option<(NodeId, Vec<Node>, Vec<Edge>)>,
    cached_positions: Option<HashMap<NodeId, egui::Pos2>>,

    // Async Layout
    layout_tx: Sender<LayoutRequest>,
    layout_rx: Receiver<LayoutResponse>,
    is_calculating: bool,
    layout_epoch: u64,
    layout_requested_epoch: Option<u64>,
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
    last_filter_settings: Option<FilterSettingsSnapshot>,

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
    trail_toolbar_expanded: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FilterSettingsSnapshot {
    show_classes: bool,
    show_functions: bool,
    show_variables: bool,
}

impl FilterSettingsSnapshot {
    fn from_settings(settings: &crate::settings::NodeGraphSettings) -> Self {
        Self {
            show_classes: settings.show_classes,
            show_functions: settings.show_functions,
            show_variables: settings.show_variables,
        }
    }
}

pub struct NodeGraphViewResponse {
    pub clicked_node: Option<NodeId>,
    pub view_state_dirty: bool,
}

impl NodeGraphView {
    pub fn new(event_bus: codestory_events::EventBus) -> Self {
        let (req_tx, req_rx) = channel::<LayoutRequest>();
        let (res_tx, res_rx) = channel::<LayoutResponse>();

        // Spawn layout worker
        thread::spawn(move || {
            while let Ok(mut req) = req_rx.recv() {
                // Coalesce bursty updates (e.g., depth slider changes): keep only the most recent
                // queued request and drop intermediate ones.
                while let Ok(next) = req_rx.try_recv() {
                    req = next;
                }

                let (epoch, root, nodes, edges, algorithm, direction) = req;
                let mut model = GraphModel::new();
                model.root = root;
                for node in &nodes {
                    model.add_node(node.clone());
                }
                for edge in &edges {
                    model.add_edge(edge.clone());
                }

                let to_node_positions = |positions: &HashMap<codestory_graph::NodeIndex, (f32, f32)>| {
                    // Keep the requested root node stable at the origin (0,0) when possible.
                    // This prevents tiny graphs from starting "far away" and needing lots of panning.
                    let (shift_x, shift_y) = root
                        .and_then(|root_id| model.node_map.get(&root_id))
                        .and_then(|root_idx| positions.get(root_idx))
                        .copied()
                        .unwrap_or((0.0, 0.0));

                    let mut result = HashMap::new();
                    for (idx, (x, y)) in positions {
                        if let Some(node) = model.graph.node_weight(*idx) {
                            result.insert(node.id, egui::pos2(*x - shift_x, *y - shift_y));
                        }
                    }
                    result
                };

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
                                let _ = res_tx.send((epoch, false, to_node_positions(positions)));
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

                let _ = res_tx.send((epoch, true, to_node_positions(&positions)));
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
            graph_context_menu_pos: None,
            last_node_rects_graph: HashMap::new(),
            last_view_state: GraphViewState::default(),
            cached_data: None,
            cached_positions: None,
            layout_tx: req_tx,
            layout_rx: res_rx,
            is_calculating: false,
            layout_epoch: 0,
            layout_requested_epoch: None,
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
            last_filter_settings: None,
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
            trail_toolbar_expanded: true,
        }
    }

    fn invalidate_layout(&mut self) {
        self.layout_epoch = self.layout_epoch.wrapping_add(1);
        self.layout_requested_epoch = None;
        self.cached_positions = None;
        self.is_calculating = false;
        self.anim_start_positions = None;
        self.anim_target_positions = None;
        self.anim_started_at = None;
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
                mode,
                target_id,
                node_filter,
            } => {
                if let Some((root_id, _, _)) = &self.cached_data {
                    let root_id = *root_id;
                    if let Some(storage) = storage {
                        let trail_config = codestory_core::TrailConfig {
                            root_id,
                            mode: *mode,
                            target_id: *target_id,
                            depth: *depth,
                            direction: *direction,
                            edge_filter: edge_filter.clone(),
                            node_filter: node_filter.clone(),
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
            mode: codestory_core::TrailMode::Neighborhood,
            target_id: None,
            depth: settings.trail_depth,
            direction: settings.trail_direction,
            edge_filter: settings.trail_edge_filter.clone(),
            node_filter: Vec::new(),
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
        // Force re-layout on new data (and ignore any stale async layout responses).
        self.invalidate_layout();

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

        let center_visible_id = resolve_visible(center_node_id).unwrap_or(center_node_id);

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

        // Trigger async layout calculation if needed, but only over the *visible* graph.
        //
        // The raw storage graph can contain many member nodes that are rendered inside their
        // parent cards (not as standalone nodes). Including those in layout pushes visible
        // nodes far apart "for no reason" (from the user's perspective).
        if settings.auto_layout
            && self.cached_positions.is_none()
            && self.layout_requested_epoch != Some(self.layout_epoch)
        {
            self.is_calculating = true;
            self.layout_requested_epoch = Some(self.layout_epoch);

            let layout_nodes: Vec<Node> = self
                .uml_nodes
                .iter()
                .map(|uml| {
                    self.node_lookup.get(&uml.id).cloned().unwrap_or_else(|| Node {
                        id: uml.id,
                        kind: uml.kind,
                        serialized_name: uml.label.clone(),
                        qualified_name: None,
                        canonical_id: None,
                        file_node_id: None,
                        start_line: None,
                        start_col: None,
                        end_line: None,
                        end_col: None,
                    })
                })
                .collect();

            let layout_edges: Vec<Edge> = self
                .current_edges
                .iter()
                .map(|e| Edge {
                    id: e.id,
                    source: e.source,
                    target: e.target,
                    kind: e.kind,
                    file_node_id: None,
                    line: None,
                    resolved_source: None,
                    resolved_target: None,
                    confidence: None,
                })
                .collect();

            let _ = self.layout_tx.send((
                self.layout_epoch,
                Some(center_visible_id),
                layout_nodes,
                layout_edges,
                self.current_layout_algorithm,
                self.current_layout_direction,
            ));
        }

        for (i, node) in self.uml_nodes.iter().enumerate() {
            let id = node.id;
            let pos = if let Some(positions) = &self.cached_positions {
                positions.get(&id).copied().unwrap_or_else(|| {
                    if id == center_visible_id {
                        egui::pos2(0.0, 0.0)
                    } else {
                        let angle = (i as f32)
                            * (std::f32::consts::PI * 2.0 / (self.uml_nodes.len() as f32));
                        let radius = 250.0;
                        egui::pos2(angle.cos() * radius, angle.sin() * radius)
                    }
                })
            } else if id == center_visible_id {
                egui::pos2(0.0, 0.0)
            } else {
                let angle =
                    (i as f32) * (std::f32::consts::PI * 2.0 / (self.uml_nodes.len() as f32));
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
        while let Ok((epoch, is_final, positions)) = self.layout_rx.try_recv() {
            if epoch != self.layout_epoch {
                continue;
            }

            self.is_calculating = !is_final;
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
        let filter_snapshot = FilterSettingsSnapshot::from_settings(settings);
        if self.last_filter_settings != Some(filter_snapshot) {
            self.last_filter_settings = Some(filter_snapshot);
            rebuild_needed = true;
        }

        let full_rect = ui.available_rect_before_wrap();
        // Force the ui to take up the whole remaining space
        ui.allocate_rect(full_rect, egui::Sense::hover());

        // Graph viewport (Sourcetrail-style: controls are overlays inside the viewport).
        ui.scope_builder(egui::UiBuilder::new().max_rect(full_rect), |ui| {
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
                let pos = ui
                    .input(|i| i.pointer.interact_pos())
                    .unwrap_or(snarl_rect.center());
                if let Some(info) = self.edge_overlay.hovered_edge_info().cloned() {
                    self.edge_context_menu = Some(EdgeContextMenu { pos, info });
                    self.graph_context_menu_pos = None;
                } else if output.interaction.hovered_node.is_none() {
                    // Right-click on empty background: show a generic graph context menu.
                    self.edge_context_menu = None;
                    self.graph_context_menu_pos = Some(pos);
                } else {
                    // Let GraphCanvas handle node/member context menus.
                    self.edge_context_menu = None;
                    self.graph_context_menu_pos = None;
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

            if let Some(pos) = self.graph_context_menu_pos {
                let mut close_menu = false;
                egui::Area::new("graph_context_menu".into())
                    .fixed_pos(pos)
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label("Graph");
                            ui.separator();

                            if ui.button("Back").clicked() {
                                self.event_bus.publish(Event::HistoryBack);
                                close_menu = true;
                            }
                            if ui.button("Forward").clicked() {
                                self.event_bus.publish(Event::HistoryForward);
                                close_menu = true;
                            }

                            ui.separator();

                            if ui.button("Zoom to Fit").clicked() {
                                self.event_bus.publish(Event::ZoomToFit);
                                close_menu = true;
                            }
                            if ui.button("Reset Zoom (0)").clicked() {
                                self.event_bus.publish(Event::ZoomReset);
                                close_menu = true;
                            }

                            ui.separator();

                            if ui.button("Save to Clipboard").clicked() {
                                copy_image = self.build_graph_image();
                                close_menu = true;
                            }
                            if ui.button("Save As Image...").clicked() {
                                export_image = true;
                                close_menu = true;
                            }

                            ui.separator();

                            let mut show_minimap = settings.show_minimap;
                            if ui.checkbox(&mut show_minimap, "Show minimap").changed() {
                                self.event_bus.publish(Event::SetShowMinimap(show_minimap));
                                close_menu = true;
                            }

                            if ui.button("Graph Settings...").clicked() {
                                self.show_toolbar_panel = true;
                                close_menu = true;
                            }

                            ui.separator();

                            if ui.button("Expand All").clicked() {
                                self.event_bus.publish(Event::ExpandAll);
                                close_menu = true;
                            }
                            if ui.button("Collapse All").clicked() {
                                self.event_bus.publish(Event::CollapseAll);
                                close_menu = true;
                            }
                        });
                    });

                if close_menu {
                    self.graph_context_menu_pos = None;
                }
            }

            if !ui.ctx().wants_keyboard_input() {
                let reset_zoom = ui.input(|i| i.key_pressed(egui::Key::Num0));
                if reset_zoom {
                    self.event_bus.publish(Event::ZoomReset);
                }

                // Sourcetrail graph shortcuts:
                // - Shift+W / Shift+S zoom
                // - Ctrl/Cmd+Shift+Up/Down zoom
                // - WASD pan
                if ui.input(|i| !i.modifiers.command && !i.modifiers.alt && i.modifiers.shift) {
                    if ui.input(|i| i.key_pressed(egui::Key::W)) {
                        self.event_bus.publish(Event::ZoomIn);
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::S)) {
                        self.event_bus.publish(Event::ZoomOut);
                    }
                }

                if ui.input(|i| i.modifiers.command && i.modifiers.shift) {
                    if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                        self.event_bus.publish(Event::ZoomIn);
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                        self.event_bus.publish(Event::ZoomOut);
                    }
                }

                // Continuous WASD pan when no other modifier is held.
                let pan_delta = ui.input(|i| {
                    if i.modifiers.command || i.modifiers.alt || i.modifiers.shift {
                        return egui::Vec2::ZERO;
                    }
                    let mut dx = 0.0;
                    let mut dy = 0.0;
                    if i.key_down(egui::Key::W) {
                        dy += 1.0;
                    }
                    if i.key_down(egui::Key::S) {
                        dy -= 1.0;
                    }
                    if i.key_down(egui::Key::A) {
                        dx += 1.0;
                    }
                    if i.key_down(egui::Key::D) {
                        dx -= 1.0;
                    }
                    if dx == 0.0 && dy == 0.0 {
                        return egui::Vec2::ZERO;
                    }
                    let dt = i.stable_dt.min(0.05);
                    let speed = 600.0; // points/second
                    egui::vec2(dx, dy) * (speed * dt)
                });
                if pan_delta != egui::Vec2::ZERO {
                    self.graph_canvas.pan_by(pan_delta);
                    view_state_dirty = true;
                    ui.ctx().request_repaint();
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

            // Paint grouping overlays behind the graph (Sourcetrail-style).
            self.paint_group_overlays(&child_ui, snarl_rect, settings);
        });

        if self.ui_graph_settings_window(ui.ctx(), settings) {
            rebuild_needed = true;
        }

        // If layout settings changed (either via external events or in-frame UI updates), we want
        // to re-layout the currently cached graph rather than re-querying storage.
        let layout_settings_changed = self.last_auto_layout != settings.auto_layout
            || self.current_layout_algorithm != settings.layout_algorithm
            || self.current_layout_direction != settings.layout_direction;
        if layout_settings_changed {
            self.current_layout_algorithm = settings.layout_algorithm;
            self.current_layout_direction = settings.layout_direction;
            self.invalidate_layout();
            rebuild_needed = true;
        }

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
                    .add_filter("PNG", &["png"])
                    .add_filter("JPEG", &["jpg", "jpeg"])
                    .add_filter("BMP", &["bmp"])
                    .save_file()
                {
                    let path = ensure_graph_image_extension(path);
                    match write_color_image(&path, &image) {
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

    fn ui_graph_settings_window(
        &mut self,
        ctx: &egui::Context,
        settings: &crate::settings::NodeGraphSettings,
    ) -> bool {
        let mut rebuild_needed = false;
        let mut invalidate_layout_requested = false;

        if self.show_toolbar_panel {
            egui::Window::new("Graph Settings")
                .collapsible(false)
                .resizable(false)
                .open(&mut self.show_toolbar_panel)
                .show(ctx, |ui| {
                    ui.label("Layout");
                    egui::ComboBox::from_id_salt("layout_selector")
                        .selected_text(format!("{:?}", settings.layout_algorithm))
                        .show_ui(ui, |ui| {
                            for (alg, label) in [
                                (codestory_events::LayoutAlgorithm::ForceDirected, "Force Directed"),
                                (codestory_events::LayoutAlgorithm::Radial, "Radial"),
                                (codestory_events::LayoutAlgorithm::Grid, "Grid"),
                                (codestory_events::LayoutAlgorithm::Hierarchical, "Hierarchical"),
                            ] {
                                if ui
                                    .selectable_label(settings.layout_algorithm == alg, label)
                                    .clicked()
                                {
                                    self.event_bus.publish(Event::SetLayoutMethod(alg));
                                }
                            }
                        });

                    if settings.layout_algorithm == codestory_events::LayoutAlgorithm::Hierarchical {
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
                                invalidate_layout_requested = true;
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
                                invalidate_layout_requested = true;
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

            if invalidate_layout_requested {
                self.invalidate_layout();
                rebuild_needed = true;
            }
        }

        rebuild_needed
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

    fn ui_trail_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        parent_rect: egui::Rect,
        settings: &mut crate::settings::NodeGraphSettings,
    ) -> bool {
        // Sourcetrail-style: a compact top-left cluster with
        // - caret (collapses/expands trail controls)
        // - grouping segmented pill (always visible)
        // - vertical rail of trail buttons + depth slider when expanded
        let palette = self.style_resolver.palette();
        let margin = 10.0;
        let gap = 4.0;
        let segment = 40.0;
        let slider_height = 250.0;
        let pos = parent_rect.min + egui::vec2(margin, margin);

        let is_light = is_light_color(palette.background);
        let base_fill = adjust_color(
            palette.node_default_fill,
            is_light,
            if is_light { 0.12 } else { 0.06 },
        );
        let border = adjust_color(
            palette.node_section_border,
            is_light,
            if is_light { 0.25 } else { 0.10 },
        );
        let divider = adjust_color(
            palette.node_section_border,
            is_light,
            if is_light { 0.18 } else { 0.06 },
        );
        let icon_color = if is_light {
            egui::Color32::WHITE
        } else {
            palette.node_default_text
        };

        let mut settings_changed = false;

        let publish_trail_config = |event_bus: &codestory_events::EventBus,
                                    depth: u32,
                                    direction: TrailDirection,
                                    edge_filter: Vec<EdgeKind>| {
            event_bus.publish(Event::TrailConfigChange {
                depth,
                direction,
                edge_filter,
                mode: codestory_core::TrailMode::Neighborhood,
                target_id: None,
                node_filter: Vec::new(),
            });
        };

        let depth_to_slider = |depth: u32| -> u32 {
            if depth == 0 {
                20
            } else {
                (depth.saturating_sub(1)).min(19)
            }
        };

        let slider_to_depth = |slider: u32| -> u32 {
            if slider == 20 {
                0
            } else {
                slider + 1
            }
        };

        let paint_reference_icon = |painter: &egui::Painter,
                                    rect: egui::Rect,
                                    color: egui::Color32,
                                    incoming: bool| {
            let stroke = egui::Stroke::new(2.0, color);
            let w = rect.width();
            let h = rect.height();
            let pad_x = w * 0.22;
            let pad_y = h * 0.22;
            let node_r = (w * 0.08).max(2.0);
            let square = (w * 0.16).max(4.0);

            let left_x = rect.left() + pad_x;
            let right_x = rect.right() - pad_x;
            let y0 = rect.center().y;
            let y_top = rect.top() + pad_y;
            let y_bot = rect.bottom() - pad_y;

            let source = if incoming {
                egui::pos2(right_x, y0)
            } else {
                egui::pos2(left_x, y0)
            };
            let t1 = if incoming {
                egui::pos2(left_x, y_top)
            } else {
                egui::pos2(right_x, y_top)
            };
            let t2 = if incoming {
                egui::pos2(left_x, y_bot)
            } else {
                egui::pos2(right_x, y_bot)
            };

            painter.line_segment([source, t1], stroke);
            painter.line_segment([source, t2], stroke);
            painter.circle_filled(source, node_r, color);
            for target in [t1, t2] {
                let r = egui::Rect::from_center_size(target, egui::vec2(square, square));
                painter.rect_stroke(r, 1.0, stroke, egui::StrokeKind::Middle);
            }
        };

        egui::Area::new("graph_trail_toolbar".into())
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                let origin = ui.cursor().min;

                let (rail_height, show_controls) = if self.trail_toolbar_expanded {
                    (segment * 4.0 + slider_height, true)
                } else {
                    (segment, false)
                };
                let rail_rect =
                    egui::Rect::from_min_size(origin, egui::vec2(segment, rail_height));
                let grouping_rect = egui::Rect::from_min_size(
                    egui::pos2(rail_rect.max.x + gap, rail_rect.min.y),
                    egui::vec2(segment * 2.0, segment),
                );

                // Reserve interaction space for both controls.
                ui.allocate_rect(rail_rect.union(grouping_rect), egui::Sense::hover());

                let painter = ui.painter();

                // ---- Rail (caret + quick buttons + depth slider) ----
                let rounding = 10u8;
                painter.rect_filled(rail_rect, egui::CornerRadius::same(rounding), base_fill);
                painter.rect_stroke(
                    rail_rect,
                    egui::CornerRadius::same(rounding),
                    egui::Stroke::new(1.0, border),
                    egui::StrokeKind::Middle,
                );

                let seg_rect = |idx: usize| -> egui::Rect {
                    egui::Rect::from_min_size(
                        egui::pos2(rail_rect.min.x, rail_rect.min.y + segment * idx as f32),
                        egui::vec2(segment, segment),
                    )
                };

                // Segment fill for hover/press/selected.
                let seg_fill = |resp: &egui::Response, selected: bool| -> egui::Color32 {
                    let mut fill = base_fill;
                    if selected {
                        fill = adjust_color(fill, is_light, 0.10);
                    }
                    if resp.hovered() {
                        fill = adjust_color(fill, is_light, 0.06);
                    }
                    if resp.is_pointer_button_down_on() {
                        fill = adjust_color(fill, is_light, 0.14);
                    }
                    fill
                };

                let caret_rect = seg_rect(0);
                let caret_id = ui.id().with("graph_trail_caret");
                let caret_resp = ui
                    .interact(caret_rect, caret_id, egui::Sense::click())
                    .on_hover_text("Toggle custom trail controls");

                if caret_resp.clicked() {
                    self.trail_toolbar_expanded = !self.trail_toolbar_expanded;
                }

                let caret_fill = seg_fill(&caret_resp, self.trail_toolbar_expanded);
                let caret_rounding = if show_controls {
                    egui::CornerRadius {
                        nw: rounding,
                        ne: rounding,
                        sw: 0,
                        se: 0,
                    }
                } else {
                    egui::CornerRadius::same(rounding)
                };
                painter.rect_filled(caret_rect, caret_rounding, caret_fill);

                // Caret icon matches Sourcetrail: up when expanded, down when collapsed.
                let caret_icon = if self.trail_toolbar_expanded {
                    ph::CARET_UP
                } else {
                    ph::CARET_DOWN
                };
                painter.text(
                    caret_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    caret_icon,
                    egui::FontId::proportional(15.0),
                    icon_color,
                );

                if show_controls {
                    // Custom Trail dialog button.
                    let dialog_rect = seg_rect(1);
                    let dialog_id = ui.id().with("graph_trail_dialog");
                    let dialog_resp = ui
                        .interact(dialog_rect, dialog_id, egui::Sense::click())
                        .on_hover_text("Custom trail dialog");
                    painter.rect_filled(dialog_rect, 0.0, seg_fill(&dialog_resp, false));
                    painter.text(
                        dialog_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        ph::SLIDERS_HORIZONTAL,
                        egui::FontId::proportional(15.0),
                        icon_color,
                    );
                    if dialog_resp.clicked() {
                        self.event_bus.publish(Event::OpenCustomTrailDialog);
                    }

                    // Quick referenced / referencing buttons (Outgoing / Incoming).
                    let outgoing_rect = seg_rect(2);
                    let outgoing_id = ui.id().with("graph_trail_outgoing");
                    let outgoing_selected = settings.trail_direction == TrailDirection::Outgoing;
                    let outgoing_resp = ui
                        .interact(outgoing_rect, outgoing_id, egui::Sense::click())
                        .on_hover_text("All referenced");
                    painter.rect_filled(
                        outgoing_rect,
                        0.0,
                        seg_fill(&outgoing_resp, outgoing_selected),
                    );
                    paint_reference_icon(painter, outgoing_rect.shrink(8.0), icon_color, false);
                    if outgoing_resp.clicked() {
                        settings.trail_direction = TrailDirection::Outgoing;
                        settings_changed = true;
                        publish_trail_config(
                            &self.event_bus,
                            settings.trail_depth,
                            settings.trail_direction,
                            settings.trail_edge_filter.clone(),
                        );
                    }

                    let incoming_rect = seg_rect(3);
                    let incoming_id = ui.id().with("graph_trail_incoming");
                    let incoming_selected = settings.trail_direction == TrailDirection::Incoming;
                    let incoming_resp = ui
                        .interact(incoming_rect, incoming_id, egui::Sense::click())
                        .on_hover_text("All referencing");
                    painter.rect_filled(
                        incoming_rect,
                        0.0,
                        seg_fill(&incoming_resp, incoming_selected),
                    );
                    paint_reference_icon(painter, incoming_rect.shrink(8.0), icon_color, true);
                    if incoming_resp.clicked() {
                        settings.trail_direction = TrailDirection::Incoming;
                        settings_changed = true;
                        publish_trail_config(
                            &self.event_bus,
                            settings.trail_depth,
                            settings.trail_direction,
                            settings.trail_edge_filter.clone(),
                        );
                    }

                    // Depth slider segment (bottom, rounded corners).
                    let slider_rect = egui::Rect::from_min_max(
                        egui::pos2(rail_rect.min.x, rail_rect.min.y + segment * 4.0),
                        rail_rect.max,
                    );
                    let slider_id = ui.id().with("graph_trail_depth");
                    let slider_resp = ui.interact(
                        slider_rect,
                        slider_id,
                        egui::Sense::click_and_drag(),
                    );
                    let slider_rounding = egui::CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: rounding,
                        se: rounding,
                    };
                    painter.rect_filled(
                        slider_rect,
                        slider_rounding,
                        seg_fill(&slider_resp, false),
                    );

                    // Separator lines between rail segments.
                    for idx in 1..=4 {
                        let y = rail_rect.min.y + segment * idx as f32;
                        painter.hline(
                            rail_rect.x_range(),
                            y,
                            egui::Stroke::new(1.0, divider),
                        );
                    }

                    // Slider: value label + track + knob.
                    let mut slider_pos = depth_to_slider(settings.trail_depth);
                    let label = if slider_pos == 20 {
                        "∞".to_string()
                    } else {
                        (slider_pos + 1).to_string()
                    };
                    painter.text(
                        egui::pos2(slider_rect.center().x, slider_rect.min.y + 18.0),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(18.0),
                        icon_color,
                    );

                    let track_top = slider_rect.min.y + 38.0;
                    let track_bottom = slider_rect.max.y - 22.0;
                    let track_x = slider_rect.center().x;
                    painter.line_segment(
                        [
                            egui::pos2(track_x, track_top),
                            egui::pos2(track_x, track_bottom),
                        ],
                        egui::Stroke::new(2.0, icon_color),
                    );

                    let t = (slider_pos as f32 / 20.0).clamp(0.0, 1.0);
                    let knob_y = track_bottom - t * (track_bottom - track_top);
                    let knob_center = egui::pos2(track_x, knob_y);
                    painter.circle_filled(knob_center, 8.0, icon_color);

                    let mut new_slider_pos = slider_pos;
                    if slider_resp.dragged() || slider_resp.clicked() {
                        if let Some(pointer) = ui.ctx().pointer_latest_pos() {
                            let y = pointer.y.clamp(track_top, track_bottom);
                            let tt = ((track_bottom - y) / (track_bottom - track_top))
                                .clamp(0.0, 1.0);
                            new_slider_pos = (tt * 20.0).round() as u32;
                        }
                    }

                    if new_slider_pos != slider_pos {
                        slider_pos = new_slider_pos;
                        let new_depth = slider_to_depth(slider_pos);
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
                }

                // ---- Grouping pill (always visible) ----
                painter.rect_filled(
                    grouping_rect,
                    egui::CornerRadius::same(rounding),
                    base_fill,
                );
                painter.rect_stroke(
                    grouping_rect,
                    egui::CornerRadius::same(rounding),
                    egui::Stroke::new(1.0, border),
                    egui::StrokeKind::Middle,
                );

                let left_seg = egui::Rect::from_min_max(
                    grouping_rect.min,
                    egui::pos2(grouping_rect.center().x, grouping_rect.max.y),
                );
                let right_seg = egui::Rect::from_min_max(
                    egui::pos2(grouping_rect.center().x, grouping_rect.min.y),
                    grouping_rect.max,
                );

                // Divider between the two grouping segments.
                painter.vline(
                    grouping_rect.center().x,
                    grouping_rect.y_range(),
                    egui::Stroke::new(1.0, divider),
                );

                let ns_id = ui.id().with("graph_group_namespace");
                let ns_resp = ui
                    .interact(left_seg, ns_id, egui::Sense::click())
                    .on_hover_text("Group by namespace/package");
                let file_id = ui.id().with("graph_group_file");
                let file_resp = ui
                    .interact(right_seg, file_id, egui::Sense::click())
                    .on_hover_text("Group by file");

                let ns_fill = seg_fill(&ns_resp, settings.group_by_namespace);
                let file_fill = seg_fill(&file_resp, settings.group_by_file);
                painter.rect_filled(
                    left_seg,
                    egui::CornerRadius {
                        nw: rounding,
                        ne: 0,
                        sw: rounding,
                        se: 0,
                    },
                    ns_fill,
                );
                painter.rect_filled(
                    right_seg,
                    egui::CornerRadius {
                        nw: 0,
                        ne: rounding,
                        sw: 0,
                        se: rounding,
                    },
                    file_fill,
                );

                painter.text(
                    left_seg.center(),
                    egui::Align2::CENTER_CENTER,
                    ph::ARROW_SQUARE_RIGHT,
                    egui::FontId::proportional(15.0),
                    icon_color,
                );
                painter.text(
                    right_seg.center(),
                    egui::Align2::CENTER_CENTER,
                    ph::FILE,
                    egui::FontId::proportional(15.0),
                    icon_color,
                );

                if ns_resp.clicked() {
                    settings.group_by_namespace = !settings.group_by_namespace;
                    settings_changed = true;
                    self.event_bus
                        .publish(Event::SetGroupByNamespace(settings.group_by_namespace));
                }

                if file_resp.clicked() {
                    settings.group_by_file = !settings.group_by_file;
                    settings_changed = true;
                    self.event_bus
                        .publish(Event::SetGroupByFile(settings.group_by_file));
                }
            });

        settings_changed
    }


    fn ui_zoom_cluster(&mut self, ui: &mut egui::Ui, parent_rect: egui::Rect) {
        // Sourcetrail-style: only +/- zoom buttons in the bottom-left corner.
        let palette = self.style_resolver.palette();
        let margin = 10.0;
        let segment = 40.0;
        let pos = egui::pos2(
            parent_rect.min.x + margin,
            parent_rect.max.y - margin - segment * 2.0,
        );

        let is_light = is_light_color(palette.background);
        let base_fill =
            adjust_color(palette.node_default_fill, is_light, if is_light { 0.12 } else { 0.06 });
        let border =
            adjust_color(palette.node_section_border, is_light, if is_light { 0.25 } else { 0.10 });
        let divider =
            adjust_color(palette.node_section_border, is_light, if is_light { 0.18 } else { 0.06 });
        let icon_color = if is_light {
            egui::Color32::WHITE
        } else {
            palette.node_default_text
        };

        egui::Area::new("graph_zoom_cluster".into())
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                let origin = ui.cursor().min;
                let rect = egui::Rect::from_min_size(origin, egui::vec2(segment, segment * 2.0));
                ui.allocate_rect(rect, egui::Sense::hover());

                let painter = ui.painter();
                let rounding = 10u8;
                painter.rect_filled(rect, egui::CornerRadius::same(rounding), base_fill);
                painter.rect_stroke(
                    rect,
                    egui::CornerRadius::same(rounding),
                    egui::Stroke::new(1.0, border),
                    egui::StrokeKind::Middle,
                );

                let seg_fill = |resp: &egui::Response| -> egui::Color32 {
                    let mut fill = base_fill;
                    if resp.hovered() {
                        fill = adjust_color(fill, is_light, 0.06);
                    }
                    if resp.is_pointer_button_down_on() {
                        fill = adjust_color(fill, is_light, 0.14);
                    }
                    fill
                };

                let plus_rect = egui::Rect::from_min_size(rect.min, egui::vec2(segment, segment));
                let minus_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x, rect.min.y + segment),
                    egui::vec2(segment, segment),
                );

                painter.hline(
                    rect.x_range(),
                    rect.min.y + segment,
                    egui::Stroke::new(1.0, divider),
                );

                let plus_id = ui.id().with("graph_zoom_in");
                let plus_resp = ui
                    .interact(plus_rect, plus_id, egui::Sense::click())
                    .on_hover_text("Zoom in");
                painter.rect_filled(
                    plus_rect,
                    egui::CornerRadius {
                        nw: rounding,
                        ne: rounding,
                        sw: 0,
                        se: 0,
                    },
                    seg_fill(&plus_resp),
                );
                painter.text(
                    plus_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    ph::PLUS,
                    egui::FontId::proportional(16.0),
                    icon_color,
                );
                if plus_resp.clicked() {
                    self.event_bus.publish(Event::ZoomIn);
                }

                let minus_id = ui.id().with("graph_zoom_out");
                let minus_resp = ui
                    .interact(minus_rect, minus_id, egui::Sense::click())
                    .on_hover_text("Zoom out");
                painter.rect_filled(
                    minus_rect,
                    egui::CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: rounding,
                        se: rounding,
                    },
                    seg_fill(&minus_resp),
                );
                painter.text(
                    minus_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    ph::MINUS,
                    egui::FontId::proportional(16.0),
                    icon_color,
                );
                if minus_resp.clicked() {
                    self.event_bus.publish(Event::ZoomOut);
                }
            });
    }

    fn ui_legend_button(
        &mut self,
        ui: &mut egui::Ui,
        parent_rect: egui::Rect,
        settings: &crate::settings::NodeGraphSettings,
    ) {
        let palette = self.style_resolver.palette();
        let margin = 10.0;
        let size = 32.0;
        let reserve_y = if settings.show_minimap {
            // Minimap is 100px tall and already has a 10px bottom margin; reserve space above it.
            100.0 + margin
        } else {
            0.0
        };
        let button_pos = egui::pos2(
            parent_rect.max.x - margin - size,
            parent_rect.max.y - margin - reserve_y - size,
        );

        let is_light = is_light_color(palette.background);
        let base_fill =
            adjust_color(palette.node_default_fill, is_light, if is_light { 0.12 } else { 0.06 });
        let border =
            adjust_color(palette.node_section_border, is_light, if is_light { 0.25 } else { 0.10 });
        let icon_color = if is_light {
            egui::Color32::WHITE
        } else {
            palette.node_default_text
        };

        egui::Area::new("graph_legend_button".into())
            .order(egui::Order::Foreground)
            .fixed_pos(button_pos)
            .show(ui.ctx(), |ui| {
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
                let resp = resp.on_hover_text("Toggle legend");

                let mut fill = base_fill;
                if settings.show_legend {
                    fill = adjust_color(fill, is_light, 0.10);
                }
                if resp.hovered() {
                    fill = adjust_color(fill, is_light, 0.06);
                }
                if resp.is_pointer_button_down_on() {
                    fill = adjust_color(fill, is_light, 0.14);
                }

                let painter = ui.painter();
                let radius = size * 0.5;
                painter.circle_filled(rect.center(), radius, fill);
                painter.circle_stroke(rect.center(), radius, egui::Stroke::new(1.0, border));
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    ph::QUESTION,
                    egui::FontId::proportional(16.0),
                    icon_color,
                );

                if resp.clicked() {
                    self.event_bus
                        .publish(Event::SetShowLegend(!settings.show_legend));
                }
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

    fn paint_group_overlays(
        &self,
        ui: &egui::Ui,
        viewport_rect: egui::Rect,
        settings: &crate::settings::NodeGraphSettings,
    ) {
        if !settings.group_by_file && !settings.group_by_namespace {
            return;
        }
        if self.last_node_rects_graph.is_empty() {
            return;
        }

        let bg_layer = egui::LayerId::new(egui::Order::Background, ui.layer_id().id);
        let mut painter = ui.ctx().layer_painter(bg_layer);
        painter.set_clip_rect(viewport_rect);

        let palette = self.style_resolver.palette();
        let is_light = is_light_color(palette.background);

        let mut uml_by_id: HashMap<NodeId, &UmlNode> = HashMap::new();
        for uml in &self.uml_nodes {
            uml_by_id.insert(uml.id, uml);
        }

        let with_alpha = |color: egui::Color32, alpha: u8| -> egui::Color32 {
            let (r, g, b, _) = color.to_tuple();
            egui::Color32::from_rgba_unmultiplied(r, g, b, alpha)
        };

        let resolve_metadata_node_id = |mut node_id: NodeId| -> Option<NodeId> {
            if self.node_lookup.contains_key(&node_id) {
                return Some(node_id);
            }
            for _ in 0..64 {
                let Some(uml) = uml_by_id.get(&node_id) else {
                    break;
                };
                let Some(parent) = uml.parent_id else {
                    break;
                };
                if self.node_lookup.contains_key(&parent) {
                    return Some(parent);
                }
                node_id = parent;
            }
            None
        };

        let node_kind = |id: NodeId| -> Option<NodeKind> {
            uml_by_id
                .get(&id)
                .map(|n| n.kind)
                .or_else(|| self.node_lookup.get(&id).map(|n| n.kind))
        };

        let paint_group = |painter: &egui::Painter,
                           label: &str,
                           rect: egui::Rect,
                           fill: egui::Color32,
                           stroke: egui::Stroke,
                           text_color: egui::Color32| {
            painter.rect_filled(rect, egui::CornerRadius::same(16), fill);
            painter.rect_stroke(
                rect,
                egui::CornerRadius::same(16),
                stroke,
                egui::StrokeKind::Middle,
            );

            let font_id = egui::FontId::proportional(13.0);
            let galley = painter.layout_no_wrap(label.to_owned(), font_id.clone(), text_color);
            let padding = egui::vec2(12.0, 6.0);
            let label_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + 14.0, rect.min.y + 6.0),
                galley.rect.size() + padding,
            );
            let label_fill = with_alpha(stroke.color, if is_light { 36 } else { 72 });
            painter.rect_filled(label_rect, egui::CornerRadius::same(10), label_fill);
            painter.text(
                label_rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                font_id,
                text_color,
            );
        };

        if settings.group_by_file {
            let mut groups: HashMap<NodeId, egui::Rect> = HashMap::new();
            for (node_id, rect_graph) in &self.last_node_rects_graph {
                let rect_screen = self.current_transform.mul_rect(*rect_graph);
                let Some(meta_id) = resolve_metadata_node_id(*node_id) else {
                    continue;
                };
                let Some(meta) = self.node_lookup.get(&meta_id) else {
                    continue;
                };
                let file_id = if meta.kind == NodeKind::FILE {
                    Some(meta.id)
                } else {
                    meta.file_node_id
                };
                let Some(file_id) = file_id else {
                    continue;
                };
                groups
                    .entry(file_id)
                    .and_modify(|r| *r = r.union(rect_screen))
                    .or_insert(rect_screen);
            }

            let mut groups: Vec<(NodeId, egui::Rect)> = groups.into_iter().collect();
            groups.sort_by(|a, b| {
                let aa = a.1.width() * a.1.height();
                let ba = b.1.width() * b.1.height();
                ba.partial_cmp(&aa).unwrap_or(std::cmp::Ordering::Equal)
            });

            for (file_id, bounds) in groups {
                let label = self
                    .node_lookup
                    .get(&file_id)
                    .map(|n| n.serialized_name.clone())
                    .unwrap_or_else(|| format!("file {}", file_id.0));
                let label = std::path::Path::new(&label)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&label)
                    .to_string();

                let rect = bounds.expand(22.0);
                let fill = egui::Color32::from_rgba_unmultiplied(
                    120,
                    200,
                    120,
                    if is_light { 30 } else { 20 },
                );
                let stroke_color = egui::Color32::from_rgba_unmultiplied(
                    140,
                    220,
                    150,
                    if is_light { 220 } else { 200 },
                );
                let text_color =
                    adjust_color(stroke_color, is_light, if is_light { 0.55 } else { 0.25 });
                paint_group(
                    &painter,
                    &label,
                    rect,
                    fill,
                    egui::Stroke::new(2.0, stroke_color),
                    text_color,
                );
            }
        }

        if settings.group_by_namespace {
            let mut groups: HashMap<NodeId, egui::Rect> = HashMap::new();
            for (node_id, rect_graph) in &self.last_node_rects_graph {
                let rect_screen = self.current_transform.mul_rect(*rect_graph);
                let mut current = *node_id;
                for _ in 0..64 {
                    let Some(uml) = uml_by_id.get(&current) else {
                        break;
                    };
                    let Some(parent) = uml.parent_id else {
                        break;
                    };
                    if matches!(
                        node_kind(parent),
                        Some(NodeKind::NAMESPACE | NodeKind::MODULE | NodeKind::PACKAGE)
                    ) {
                        groups
                            .entry(parent)
                            .and_modify(|r| *r = r.union(rect_screen))
                            .or_insert(rect_screen);
                    }
                    current = parent;
                }
            }

            let mut groups: Vec<(NodeId, egui::Rect)> = groups.into_iter().collect();
            groups.sort_by(|a, b| {
                let aa = a.1.width() * a.1.height();
                let ba = b.1.width() * b.1.height();
                ba.partial_cmp(&aa).unwrap_or(std::cmp::Ordering::Equal)
            });

            for (ns_id, bounds) in groups {
                let label = uml_by_id
                    .get(&ns_id)
                    .map(|n| n.label.clone())
                    .or_else(|| self.node_display_name(ns_id))
                    .unwrap_or_else(|| format!("namespace {}", ns_id.0));

                let rect = bounds.expand(24.0);
                let base = palette.namespace_fill;
                let fill = with_alpha(base, if is_light { 28 } else { 18 });
                let stroke_color = with_alpha(base, if is_light { 200 } else { 180 });
                let text_color =
                    adjust_color(stroke_color, is_light, if is_light { 0.62 } else { 0.20 });
                paint_group(
                    &painter,
                    &label,
                    rect,
                    fill,
                    egui::Stroke::new(2.0, stroke_color),
                    text_color,
                );
            }
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

fn ensure_graph_image_extension(path: std::path::PathBuf) -> std::path::PathBuf {
    if path.extension().is_some() {
        path
    } else {
        path.with_extension("png")
    }
}

fn write_color_image(path: &std::path::Path, image: &egui::ColorImage) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();

    let format = match ext.as_str() {
        "jpg" | "jpeg" => image::ImageFormat::Jpeg,
        "bmp" => image::ImageFormat::Bmp,
        _ => image::ImageFormat::Png,
    };

    let width = image.size[0] as u32;
    let height = image.size[1] as u32;

    match format {
        image::ImageFormat::Png => {
            let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
            for pixel in &image.pixels {
                let (r, g, b, a) = pixel.to_tuple();
                bytes.extend_from_slice(&[r, g, b, a]);
            }
            image::save_buffer_with_format(
                path,
                &bytes,
                width,
                height,
                image::ColorType::Rgba8,
                format,
            )
            .map_err(|err| err.to_string())
        }
        image::ImageFormat::Jpeg | image::ImageFormat::Bmp => {
            // Drop alpha for formats that don't support RGBA well.
            let mut bytes = Vec::with_capacity(image.pixels.len() * 3);
            for pixel in &image.pixels {
                let (r, g, b, _) = pixel.to_tuple();
                bytes.extend_from_slice(&[r, g, b]);
            }
            image::save_buffer_with_format(
                path,
                &bytes,
                width,
                height,
                image::ColorType::Rgb8,
                format,
            )
            .map_err(|err| err.to_string())
        }
        _ => Err("Unsupported export format".to_string()),
    }
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
