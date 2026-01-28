use super::snarl_adapter::NodeGraphAdapter;
use crate::components::node_graph::edge_overlay::EdgeOverlay;
use codestory_core::{Edge, Node, NodeId, NodeKind, TrailConfig, TrailDirection};
use codestory_events::Event;
use codestory_graph::converter::NodeGraphConverter;
use codestory_graph::uml_types::UmlNode;
use codestory_graph::{
    DummyEdge, ForceDirectedLayouter, GraphModel, GridLayouter, Layouter, NestingLayouter,
    RadialLayouter,
};
use codestory_storage::Storage;
use eframe::egui;
use egui_snarl::{
    ui::{PinPlacement, SnarlStyle},
    NodeId as SnarlNodeId, Snarl,
};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

type LayoutRequest = (
    Option<NodeId>,
    Vec<Node>,
    Vec<Edge>,
    codestory_events::LayoutAlgorithm,
    codestory_core::LayoutDirection,
);

pub struct NodeGraphView {
    pub snarl: Snarl<UmlNode>,
    adapter: NodeGraphAdapter,
    style: SnarlStyle,
    event_bus: codestory_events::EventBus,
    edge_overlay: EdgeOverlay,
    current_edges: Vec<DummyEdge>,

    // Data Cache for rebuilding
    cached_data: Option<(NodeId, Vec<Node>, Vec<Edge>)>,
    cached_positions: Option<HashMap<NodeId, egui::Pos2>>,

    // Async Layout
    layout_tx: Sender<LayoutRequest>,
    layout_rx: Receiver<HashMap<NodeId, egui::Pos2>>,
    is_calculating: bool,

    // Local Filter State
    // Removed local state in favor of settings
    hidden_nodes: std::collections::HashSet<NodeId>,
    node_map: HashMap<NodeId, SnarlNodeId>,

    // State tracking
    last_auto_layout: bool,
    current_layout_algorithm: codestory_events::LayoutAlgorithm,
    current_layout_direction: codestory_core::LayoutDirection,
    _last_settings_version: u64, // To track changes (reserved for future use)

    // UI Features
    // Removed local state
    pub pending_pan_to_node: Option<NodeId>,
    view_version: u64,
    theme_flavor: catppuccin_egui::Theme,
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
                        let layouter = ForceDirectedLayouter::default();
                        layouter.execute(&model)
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

        let style = SnarlStyle {
            pin_placement: Some(PinPlacement::Outside { margin: -12.0 }),
            pin_size: Some(8.0),
            header_frame: Some(egui::Frame::NONE.inner_margin(egui::Margin {
                left: 16,
                right: 16,
                top: 4,
                bottom: 4,
            })),
            node_layout: None,
            ..Default::default()
        };

        Self {
            snarl: Snarl::new(),
            adapter: NodeGraphAdapter {
                clicked_node: None,
                node_to_focus: None,
                node_to_hide: None,
                node_to_navigate: None,
                theme: catppuccin_egui::MOCHA,
                collapse_states: std::collections::HashMap::new(),
                event_bus: event_bus.clone(),
                node_rects: std::collections::HashMap::new(),
                current_transform: egui::emath::TSTransform::default(),
                pin_info: std::collections::HashMap::new(),
            },
            style,
            event_bus,
            edge_overlay: EdgeOverlay::new(),
            current_edges: Vec::new(),
            cached_data: None,
            cached_positions: None,
            layout_tx: req_tx,
            layout_rx: res_rx,
            is_calculating: false,
            // show_classes: true,
            // show_functions: true,
            // show_variables: true,
            hidden_nodes: std::collections::HashSet::new(),
            node_map: HashMap::new(),
            last_auto_layout: true,
            current_layout_algorithm: codestory_events::LayoutAlgorithm::default(),
            current_layout_direction: codestory_core::LayoutDirection::default(),
            _last_settings_version: 0,
            // show_minimap: true,
            // show_legend: false,
            pending_pan_to_node: None,
            view_version: 0,
            theme_flavor: catppuccin_egui::MOCHA,
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
                // TODO: Implement global expand
            }
            Event::CollapseAll => {
                // TODO: Implement global collapse
            }
            Event::ZoomToFit => {
                self.view_version = self.view_version.wrapping_add(1);
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
            depth: settings.max_depth as u32,
            direction: TrailDirection::Both,
            edge_filter: Vec::new(),
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
            self.snarl = Snarl::new();
            return;
        };

        let center_node_id = *center_node_id;

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

        let filtered_edges: Vec<Edge> = edges
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

        // Clear current graph
        self.snarl = Snarl::new();

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
        let (uml_nodes, _graph_edges, pin_info) =
            converter.convert_dummies_to_uml(&bundled_nodes, &bundled_edges);

        // Store edges for EdgeOverlay
        self.current_edges = bundled_edges;

        // Store pin_info in adapter
        self.adapter.pin_info = pin_info;

        self.node_map.clear();

        for (i, node) in uml_nodes.into_iter().enumerate() {
            let id = node.id;

            // Use cached position or default circular layout while loading
            let pos = if let Some(positions) = &self.cached_positions {
                positions.get(&id).copied().unwrap_or(egui::pos2(0.0, 0.0))
            } else if id == center_node_id {
                egui::pos2(0.0, 0.0)
            } else {
                let angle =
                    (i as f32) * (std::f32::consts::PI * 2.0 / (bundled_nodes.len() as f32));
                let radius = 250.0;
                egui::pos2(angle.cos() * radius, angle.sin() * radius)
            };

            let handle = self.snarl.insert_node(pos, node);
            self.node_map.insert(id, handle);
        }

        // We do NOT add connections to Snarl anymore because we render edges manually via EdgeOverlay.

        self.last_auto_layout = settings.auto_layout;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        settings: &crate::settings::NodeGraphSettings,
        theme: catppuccin_egui::Theme,
    ) -> Option<NodeId> {
        self.theme_flavor = theme;
        self.adapter.theme = theme;

        // Sync collapse states from settings (persistence)
        self.adapter.collapse_states = settings.view_state.collapse_states.clone();

        // Update node_frame colors based on current theme
        let node_inner_margin = egui::Margin {
            left: 16,
            right: 16,
            top: 8,
            bottom: 8,
        };

        self.style.node_frame = Some(
            egui::Frame::NONE
                .fill(theme.surface0)
                .stroke(egui::Stroke::new(1.0, theme.overlay0))
                .corner_radius(6)
                .inner_margin(node_inner_margin),
        );

        let mut node_to_navigate = None;

        if let Some(node_id) = self.adapter.node_to_focus {
            self.adapter.node_to_focus = None;
            node_to_navigate = Some(node_id);
        }

        if let Some(node_id) = self.adapter.node_to_navigate {
            self.adapter.node_to_navigate = None;
            node_to_navigate = Some(node_id);
        }

        if let Some(node_id) = self.adapter.node_to_hide {
            self.adapter.node_to_hide = None;
            self.hidden_nodes.insert(node_id);
            self.rebuild_graph(settings);
        }

        self.adapter.clicked_node = None;

        // Poll for layout results
        // Apply Layout Results
        while let Ok(positions) = self.layout_rx.try_recv() {
            self.is_calculating = false;
            self.cached_positions = Some(positions.clone());
            self.rebuild_graph(settings);
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

        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Node Graph").strong());
                if self.is_calculating {
                    ui.spinner();
                }
                ui.separator();

                // Toolbar
                ui.label("Depth:");
                let mut depth = settings.max_depth as u32;
                if ui
                    .add(egui::DragValue::new(&mut depth).range(1..=10))
                    .changed()
                {
                    self.event_bus.publish(Event::SetTrailDepth(depth));
                }
                ui.separator();

                // Layout Selector
                egui::ComboBox::from_id_salt("layout_selector")
                    .selected_text(format!("{:?}", settings.layout_algorithm))
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                settings.layout_algorithm
                                    == codestory_events::LayoutAlgorithm::ForceDirected,
                                "Force Directed",
                            )
                            .clicked()
                        {
                            self.event_bus.publish(Event::SetLayoutMethod(
                                codestory_events::LayoutAlgorithm::ForceDirected,
                            ));
                        }
                        if ui
                            .selectable_label(
                                settings.layout_algorithm
                                    == codestory_events::LayoutAlgorithm::Radial,
                                "Radial",
                            )
                            .clicked()
                        {
                            self.event_bus.publish(Event::SetLayoutMethod(
                                codestory_events::LayoutAlgorithm::Radial,
                            ));
                        }
                        if ui
                            .selectable_label(
                                settings.layout_algorithm
                                    == codestory_events::LayoutAlgorithm::Grid,
                                "Grid",
                            )
                            .clicked()
                        {
                            self.event_bus.publish(Event::SetLayoutMethod(
                                codestory_events::LayoutAlgorithm::Grid,
                            ));
                        }
                        if ui
                            .selectable_label(
                                settings.layout_algorithm
                                    == codestory_events::LayoutAlgorithm::Hierarchical,
                                "Hierarchical",
                            )
                            .clicked()
                        {
                            self.event_bus.publish(Event::SetLayoutMethod(
                                codestory_events::LayoutAlgorithm::Hierarchical,
                            ));
                        }
                    });

                ui.separator();

                // Direction Toggle (only for Hierarchical)
                if settings.layout_algorithm == codestory_events::LayoutAlgorithm::Hierarchical {
                    if ui
                        .selectable_value(
                            &mut self.current_layout_direction,
                            codestory_core::LayoutDirection::Horizontal,
                            "Horizontal",
                        )
                        .changed()
                    {
                        self.event_bus
                            .publish(codestory_events::Event::SetLayoutDirection(
                                codestory_core::LayoutDirection::Horizontal,
                            ));
                        self.cached_positions = None;
                        rebuild_needed = true;
                    }
                    if ui
                        .selectable_value(
                            &mut self.current_layout_direction,
                            codestory_core::LayoutDirection::Vertical,
                            "Vertical",
                        )
                        .changed()
                    {
                        self.event_bus
                            .publish(codestory_events::Event::SetLayoutDirection(
                                codestory_core::LayoutDirection::Vertical,
                            ));
                        self.cached_positions = None;
                        rebuild_needed = true;
                    }
                }

                ui.separator();

                ui.separator();

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
                if ui.button("Zoom to Fit").clicked() {
                    self.view_version = self.view_version.wrapping_add(1);
                }

                let mut show_minimap = settings.show_minimap;
                if ui.toggle_value(&mut show_minimap, "Minimap").changed() {
                    self.event_bus.publish(Event::SetShowMinimap(show_minimap));
                }

                let mut show_legend = settings.show_legend;
                if ui.toggle_value(&mut show_legend, "Legend").changed() {
                    self.event_bus.publish(Event::SetShowLegend(show_legend));
                }

                ui.separator();
                if ui.button("Reset View").clicked() {
                    rebuild_needed = true;
                }
            });
            ui.separator();

            let (snarl_rect, _) = ui.allocate_at_least(ui.available_size(), egui::Sense::hover());
            let mut child_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(snarl_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );

            // Clear cached screen rects before new frame rendering
            self.adapter.node_rects.clear();

            self.snarl.show(
                &mut self.adapter,
                &self.style,
                ("node_graph", self.view_version),
                &mut child_ui,
            );

            // Render custom edges - curves and arrowheads in foreground
            let snarl_id = ui.make_persistent_id(("node_graph", self.view_version));
            let fg_layer = egui::LayerId::new(egui::Order::Foreground, snarl_id);
            // IDENTITY PAINTER (Fresh for the layer, no inherited transforms)
            let mut fg_painter = ui.ctx().layer_painter(fg_layer);
            fg_painter.set_clip_rect(ui.clip_rect());

            // Supply node labels from snarl graph for edge tooltip display (Phase 9)
            {
                let mut labels = HashMap::new();
                for (snarl_id, _node) in self.snarl.node_ids() {
                    let uml = &self.snarl[snarl_id];
                    labels.insert(uml.id, uml.label.clone());
                }
                self.edge_overlay.set_node_labels(labels);
            }

            self.edge_overlay.render(
                ui,
                &fg_painter,
                &fg_painter,
                &self.current_edges,
                &self.adapter.node_rects,
                ui.clip_rect(),
                self.adapter.current_transform,
                node_inner_margin,
            );

            if settings.show_minimap {
                self.ui_minimap(ui, snarl_rect, settings);
            }

            if settings.show_legend {
                self.ui_legend(ui, snarl_rect);
            }
        });

        if rebuild_needed {
            self.rebuild_graph(settings);
        }

        self.adapter.clicked_node.or(node_to_navigate)
    }

    fn ui_minimap(
        &self,
        ui: &mut egui::Ui,
        parent_rect: egui::Rect,
        _settings: &crate::settings::NodeGraphSettings,
    ) {
        let minimap_size = egui::vec2(150.0, 100.0);
        let minimap_rect = egui::Rect::from_min_size(
            parent_rect.right_bottom() - minimap_size - egui::vec2(10.0, 10.0),
            minimap_size,
        );

        ui.painter()
            .rect_filled(minimap_rect, 5.0, self.theme_flavor.mantle);
        ui.painter().rect_stroke(
            minimap_rect,
            5.0,
            (1.0, self.theme_flavor.overlay0),
            egui::StrokeKind::Middle,
        );

        // Find bounds of snarl nodes
        let mut min = egui::pos2(f32::INFINITY, f32::INFINITY);
        let mut max = egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);

        let mut has_nodes = false;
        for (pos, _) in self.snarl.nodes_pos() {
            has_nodes = true;
            min.x = min.x.min(pos.x);
            min.y = min.y.min(pos.y);
            max.x = max.x.max(pos.x + 100.0); // Rough node size
            max.y = max.y.max(pos.y + 100.0);
        }

        if !has_nodes {
            return;
        }

        let bounds = egui::Rect::from_min_max(min, max);
        let scale = (minimap_size.x / bounds.width())
            .min(minimap_size.y / bounds.height())
            .min(1.0);
        let offset = minimap_rect.center() - bounds.center() * scale;

        for (pos, _) in self.snarl.nodes_pos() {
            let map_pos = pos * scale + offset;
            let size = egui::vec2(100.0, 40.0) * scale;
            ui.painter().rect_filled(
                egui::Rect::from_min_size(map_pos, size),
                1.0,
                egui::Color32::from_white_alpha(100),
            );
        }
    }

    fn ui_legend(&self, ui: &mut egui::Ui, parent_rect: egui::Rect) {
        let legend_size = egui::vec2(120.0, 150.0);
        let legend_rect = egui::Rect::from_min_size(
            parent_rect.left_bottom() - egui::vec2(0.0, legend_size.y + 10.0)
                + egui::vec2(10.0, 0.0),
            legend_size,
        );

        egui::Window::new("Legend")
            .fixed_pos(legend_rect.min)
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .frame(egui::Frame::window(ui.style()).fill(self.theme_flavor.crust))
            .show(ui.ctx(), |ui| {
                ui.label(egui::RichText::new("Legend").strong());
                ui.separator();

                let items = [
                    ("Class", self.theme_flavor.blue),
                    ("Struct", self.theme_flavor.teal),
                    ("Function", self.theme_flavor.yellow),
                    ("Module", self.theme_flavor.mauve),
                    ("Variable", self.theme_flavor.text),
                ];

                for (name, color) in items {
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_at_least(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, color);
                        ui.label(name);
                    });
                }
            });
    }
}
