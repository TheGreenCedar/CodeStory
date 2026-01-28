//! Custom Trail Dialog
//!
//! A dialog for configuring custom code trails with:
//! - From/To node selection
//! - Depth slider
//! - Layout direction options
//! - Node type filters
//! - Edge type filters

use crate::theme::to_egui_color;
use codestory_core::{EdgeKind, NodeId, NodeKind, TrailConfig, TrailDirection};
use codestory_events::{Event, EventBus};
use codestory_graph::{get_edge_kind_label, get_edge_style, get_kind_label, get_node_colors};

macro_rules! impl_toggle_methods {
    ($($field:ident),* $(,)?) => {
        /// Select all filters
        pub fn select_all(&mut self) {
            $(self.$field = true;)*
        }

        /// Deselect all filters
        pub fn deselect_all(&mut self) {
            $(self.$field = false;)*
        }
    };
}

/// Custom trail dialog state
pub struct CustomTrailDialog {
    /// Whether the dialog is open
    pub is_open: bool,

    /// Source node for the trail
    pub from_node: Option<NodeId>,
    pub from_node_name: String,

    /// Target node for the trail (optional)
    pub to_node: Option<NodeId>,
    pub to_node_name: String,

    /// Trail depth (1-20)
    pub depth: u32,

    /// Layout direction
    pub direction: TrailDirection,

    /// Node types to include
    pub node_filters: NodeTypeFilters,

    /// Edge types to include
    pub edge_filters: EdgeTypeFilters,

    /// Search query for from/to fields
    from_search: String,
    to_search: String,

    /// Whether to show search results
    show_from_search: bool,
    show_to_search: bool,
}

/// Node type filter checkboxes
#[derive(Debug, Clone)]
pub struct NodeTypeFilters {
    pub class: bool,
    pub struct_: bool,
    pub interface: bool,
    pub function: bool,
    pub method: bool,
    pub field: bool,
    pub variable: bool,
    pub file: bool,
    pub namespace: bool,
    pub macro_: bool,
    pub enum_: bool,
}

impl Default for NodeTypeFilters {
    fn default() -> Self {
        Self {
            class: true,
            struct_: true,
            interface: true,
            function: true,
            method: true,
            field: true,
            variable: true,
            file: false,      // Usually not wanted in trails
            namespace: false, // Usually not wanted in trails
            macro_: true,
            enum_: true,
        }
    }
}

impl NodeTypeFilters {
    /// Get enabled node kinds - available for use when building trail queries
    pub fn enabled_kinds(&self) -> Vec<NodeKind> {
        let mut kinds = Vec::new();
        if self.class {
            kinds.push(NodeKind::CLASS);
        }
        if self.struct_ {
            kinds.push(NodeKind::STRUCT);
        }
        if self.interface {
            kinds.push(NodeKind::INTERFACE);
        }
        if self.function {
            kinds.push(NodeKind::FUNCTION);
        }
        if self.method {
            kinds.push(NodeKind::METHOD);
        }
        if self.field {
            kinds.push(NodeKind::FIELD);
        }
        if self.variable {
            kinds.push(NodeKind::VARIABLE);
            kinds.push(NodeKind::GLOBAL_VARIABLE);
        }
        if self.file {
            kinds.push(NodeKind::FILE);
        }
        if self.namespace {
            kinds.push(NodeKind::NAMESPACE);
            kinds.push(NodeKind::MODULE);
            kinds.push(NodeKind::PACKAGE);
        }
        if self.macro_ {
            kinds.push(NodeKind::MACRO);
        }
        if self.enum_ {
            kinds.push(NodeKind::ENUM);
            kinds.push(NodeKind::ENUM_CONSTANT);
        }
        kinds
    }

    impl_toggle_methods!(
        class, struct_, interface, function, method, field, variable, file, namespace, macro_,
        enum_,
    );
}

/// Edge type filter checkboxes
#[derive(Debug, Clone)]
pub struct EdgeTypeFilters {
    pub call: bool,
    pub usage: bool,
    pub type_usage: bool,
    pub inheritance: bool,
    pub override_: bool,
    pub member: bool,
    pub include: bool,
    pub import: bool,
    pub macro_usage: bool,
}

impl Default for EdgeTypeFilters {
    fn default() -> Self {
        Self {
            call: true,
            usage: true,
            type_usage: true,
            inheritance: true,
            override_: true,
            member: false,  // Usually clutters trails
            include: false, // Usually not wanted
            import: false,  // Usually not wanted
            macro_usage: true,
        }
    }
}

impl EdgeTypeFilters {
    /// Get enabled edge kinds - available for use when building trail queries
    pub fn enabled_kinds(&self) -> Vec<EdgeKind> {
        let mut kinds = Vec::new();
        if self.call {
            kinds.push(EdgeKind::CALL);
        }
        if self.usage {
            kinds.push(EdgeKind::USAGE);
        }
        if self.type_usage {
            kinds.push(EdgeKind::TYPE_USAGE);
            kinds.push(EdgeKind::TYPE_ARGUMENT);
        }
        if self.inheritance {
            kinds.push(EdgeKind::INHERITANCE);
        }
        if self.override_ {
            kinds.push(EdgeKind::OVERRIDE);
        }
        if self.member {
            kinds.push(EdgeKind::MEMBER);
        }
        if self.include {
            kinds.push(EdgeKind::INCLUDE);
        }
        if self.import {
            kinds.push(EdgeKind::IMPORT);
        }
        if self.macro_usage {
            kinds.push(EdgeKind::MACRO_USAGE);
        }
        kinds
    }

    impl_toggle_methods!(
        call,
        usage,
        type_usage,
        inheritance,
        override_,
        member,
        include,
        import,
        macro_usage,
    );
}

impl Default for CustomTrailDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl CustomTrailDialog {
    pub fn new() -> Self {
        Self {
            is_open: false,
            from_node: None,
            from_node_name: String::new(),
            to_node: None,
            to_node_name: String::new(),
            depth: 3,
            direction: TrailDirection::Outgoing,
            node_filters: NodeTypeFilters::default(),
            edge_filters: EdgeTypeFilters::default(),
            from_search: String::new(),
            to_search: String::new(),
            show_from_search: false,
            show_to_search: false,
        }
    }

    /// Close the dialog
    pub fn close(&mut self) {
        self.is_open = false;
    }

    /// Build a TrailConfig from current settings
    /// Returns None if no starting node is set
    pub fn build_config(&self) -> Option<TrailConfig> {
        let root_id = self.from_node?;

        // Log useful configuration info
        tracing::debug!(
            "Building trail config. Active node filters: {:?}",
            self.node_filters.enabled_kinds()
        );

        Some(TrailConfig {
            root_id,
            depth: self.depth,
            direction: self.direction,
            edge_filter: self.edge_filters.enabled_kinds(),
            max_nodes: 500,
        })
    }

    /// Render the dialog
    /// Returns true if the "Start Trail" button was clicked
    pub fn ui(&mut self, ctx: &egui::Context, event_bus: &EventBus) -> bool {
        if !self.is_open {
            return false;
        }

        let mut start_trail = false;
        let mut should_close = false;

        egui::Window::new("Custom Trail")
            .resizable(true)
            .default_width(450.0)
            .show(ctx, |ui| {
                ui.heading("Configure Trail");

                // Close button in header
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(crate::theme::small_icon_button("✕"))
                            .on_hover_text("Close")
                            .clicked()
                        {
                            should_close = true;
                        }
                    });
                });

                ui.separator();

                // From/To nodes
                egui::Grid::new("trail_endpoints")
                    .num_columns(2)
                    .spacing([10.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("From:");
                        ui.horizontal(|ui| {
                            let response = ui.text_edit_singleline(&mut self.from_search);
                            if response.changed() {
                                self.show_from_search = !self.from_search.is_empty();
                            }
                            if self.from_node.is_some()
                                && ui.button("✕").on_hover_text("Clear").clicked()
                            {
                                self.from_node = None;
                                self.from_node_name.clear();
                                self.from_search.clear();
                            }
                        });
                        ui.end_row();

                        ui.label("To (optional):");
                        ui.horizontal(|ui| {
                            let response = ui.text_edit_singleline(&mut self.to_search);
                            if response.changed() {
                                self.show_to_search = !self.to_search.is_empty();
                            }
                            if self.to_node.is_some()
                                && ui.button("✕").on_hover_text("Clear").clicked()
                            {
                                self.to_node = None;
                                self.to_node_name.clear();
                                self.to_search.clear();
                            }
                        });
                        ui.end_row();
                    });

                ui.separator();

                // Depth and direction
                ui.horizontal(|ui| {
                    ui.label("Depth:");
                    ui.add(egui::Slider::new(&mut self.depth, 1..=20).integer());
                });

                ui.horizontal(|ui| {
                    ui.label("Direction:");
                    ui.selectable_value(
                        &mut self.direction,
                        TrailDirection::Outgoing,
                        "Outgoing →",
                    );
                    ui.selectable_value(
                        &mut self.direction,
                        TrailDirection::Incoming,
                        "← Incoming",
                    );
                    ui.selectable_value(&mut self.direction, TrailDirection::Both, "↔ Both");
                });

                ui.separator();

                // Node type filters
                ui.collapsing("Node Types", |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Select All").clicked() {
                            self.node_filters.select_all();
                        }
                        if ui.button("Deselect All").clicked() {
                            self.node_filters.deselect_all();
                        }
                    });

                    ui.separator();

                    egui::Grid::new("node_filters")
                        .num_columns(3)
                        .spacing([10.0, 5.0])
                        .show(ui, |ui| {
                            Self::node_checkbox(ui, &mut self.node_filters.class, NodeKind::CLASS);
                            Self::node_checkbox(
                                ui,
                                &mut self.node_filters.struct_,
                                NodeKind::STRUCT,
                            );
                            Self::node_checkbox(
                                ui,
                                &mut self.node_filters.interface,
                                NodeKind::INTERFACE,
                            );
                            ui.end_row();

                            Self::node_checkbox(
                                ui,
                                &mut self.node_filters.function,
                                NodeKind::FUNCTION,
                            );
                            Self::node_checkbox(
                                ui,
                                &mut self.node_filters.method,
                                NodeKind::METHOD,
                            );
                            Self::node_checkbox(ui, &mut self.node_filters.field, NodeKind::FIELD);
                            ui.end_row();

                            Self::node_checkbox(
                                ui,
                                &mut self.node_filters.variable,
                                NodeKind::VARIABLE,
                            );
                            Self::node_checkbox(ui, &mut self.node_filters.file, NodeKind::FILE);
                            Self::node_checkbox(
                                ui,
                                &mut self.node_filters.namespace,
                                NodeKind::NAMESPACE,
                            );
                            ui.end_row();

                            Self::node_checkbox(ui, &mut self.node_filters.macro_, NodeKind::MACRO);
                            Self::node_checkbox(ui, &mut self.node_filters.enum_, NodeKind::ENUM);
                            ui.end_row();
                        });
                });

                // Edge type filters
                ui.collapsing("Edge Types", |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Select All").clicked() {
                            self.edge_filters.select_all();
                        }
                        if ui.button("Deselect All").clicked() {
                            self.edge_filters.deselect_all();
                        }
                    });

                    ui.separator();

                    egui::Grid::new("edge_filters")
                        .num_columns(3)
                        .spacing([10.0, 5.0])
                        .show(ui, |ui| {
                            Self::edge_checkbox(ui, &mut self.edge_filters.call, EdgeKind::CALL);
                            Self::edge_checkbox(ui, &mut self.edge_filters.usage, EdgeKind::USAGE);
                            Self::edge_checkbox(
                                ui,
                                &mut self.edge_filters.type_usage,
                                EdgeKind::TYPE_USAGE,
                            );
                            ui.end_row();

                            Self::edge_checkbox(
                                ui,
                                &mut self.edge_filters.inheritance,
                                EdgeKind::INHERITANCE,
                            );
                            Self::edge_checkbox(
                                ui,
                                &mut self.edge_filters.override_,
                                EdgeKind::OVERRIDE,
                            );
                            Self::edge_checkbox(
                                ui,
                                &mut self.edge_filters.member,
                                EdgeKind::MEMBER,
                            );
                            ui.end_row();

                            Self::edge_checkbox(
                                ui,
                                &mut self.edge_filters.include,
                                EdgeKind::INCLUDE,
                            );
                            Self::edge_checkbox(
                                ui,
                                &mut self.edge_filters.import,
                                EdgeKind::IMPORT,
                            );
                            Self::edge_checkbox(
                                ui,
                                &mut self.edge_filters.macro_usage,
                                EdgeKind::MACRO_USAGE,
                            );
                            ui.end_row();
                        });
                });

                ui.separator();

                // Action buttons
                ui.horizontal(|ui| {
                    let can_start = self.from_node.is_some();

                    if ui
                        .add_enabled(can_start, egui::Button::new("Start Trail"))
                        .on_hover_text(if can_start {
                            "Start the trail"
                        } else {
                            "Select a From node first"
                        })
                        .clicked()
                        && let Some(from_id) = self.from_node
                        && let Some(config) = self.build_config()
                    {
                        event_bus.publish(Event::TrailModeEnter { root_id: from_id });
                        event_bus.publish(Event::TrailConfigChange {
                            depth: config.depth,
                            direction: config.direction,
                            edge_filter: config.edge_filter,
                        });
                        start_trail = true;
                        should_close = true;
                    }

                    if ui.button("Cancel").clicked() {
                        should_close = true;
                    }
                });
            });

        if should_close {
            self.is_open = false;
        }

        start_trail
    }

    fn node_checkbox(ui: &mut egui::Ui, checked: &mut bool, kind: NodeKind) {
        let colors = get_node_colors(kind);
        let color = to_egui_color(colors.fill);

        ui.horizontal(|ui| {
            ui.checkbox(checked, "");

            // Color swatch
            let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, color);

            ui.label(get_kind_label(kind));
        });
    }

    fn edge_checkbox(ui: &mut egui::Ui, checked: &mut bool, kind: EdgeKind) {
        let style = get_edge_style(kind, false, false);
        let color = to_egui_color(style.color);

        ui.horizontal(|ui| {
            ui.checkbox(checked, "");

            // Line swatch
            let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 12.0), egui::Sense::hover());
            let center_y = rect.center().y;
            ui.painter().line_segment(
                [
                    egui::pos2(rect.left(), center_y),
                    egui::pos2(rect.right(), center_y),
                ],
                egui::Stroke::new(style.width, color),
            );

            ui.label(get_edge_kind_label(kind));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_filters() {
        let filters = NodeTypeFilters::default();
        let kinds = filters.enabled_kinds();
        assert!(kinds.contains(&NodeKind::CLASS));
        assert!(kinds.contains(&NodeKind::FUNCTION));
        assert!(!kinds.contains(&NodeKind::FILE)); // Not enabled by default
    }

    #[test]
    fn test_edge_filters() {
        let filters = EdgeTypeFilters::default();
        let kinds = filters.enabled_kinds();
        assert!(kinds.contains(&EdgeKind::CALL));
        assert!(kinds.contains(&EdgeKind::INHERITANCE));
        assert!(!kinds.contains(&EdgeKind::MEMBER)); // Not enabled by default
    }

    #[test]
    fn test_build_config() {
        use codestory_core::NodeId;

        let mut dialog = CustomTrailDialog::new();
        dialog.from_node = Some(NodeId(42)); // Required for config to build
        dialog.depth = 5;
        dialog.direction = TrailDirection::Both;

        let config = dialog.build_config().unwrap();
        assert_eq!(config.depth, 5);
        assert_eq!(config.direction, TrailDirection::Both);
        assert_eq!(config.root_id, NodeId(42));
    }
}
