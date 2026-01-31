//! Custom Trail Dialog
//!
//! A dialog for configuring custom code trails with:
//! - From/To node selection
//! - Depth slider
//! - Layout direction options
//! - Node type filters
//! - Edge type filters

use crate::components::search_bar::SearchMatch;
use crate::theme::{badge, to_egui_color};
use codestory_core::{EdgeKind, NodeId, NodeKind, TrailConfig, TrailDirection};
use codestory_events::{Event, EventBus};
use codestory_graph::{get_edge_kind_label, get_edge_style, get_kind_label, get_node_colors};
use codestory_search::SearchEngine;
use codestory_storage::Storage;
use egui_phosphor::regular as ph;

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

    from_suggestions: Vec<SearchMatch>,
    to_suggestions: Vec<SearchMatch>,
    from_selected_index: usize,
    to_selected_index: usize,
    last_from_query: String,
    last_to_query: String,
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
            from_suggestions: Vec::new(),
            to_suggestions: Vec::new(),
            from_selected_index: 0,
            to_selected_index: 0,
            last_from_query: String::new(),
            last_to_query: String::new(),
        }
    }

    /// Open the dialog
    pub fn open(&mut self) {
        self.is_open = true;
    }

    /// Open the dialog and prefill the "From" node
    pub fn open_with_root(&mut self, root_id: NodeId, name: Option<String>) {
        self.is_open = true;
        self.from_node = Some(root_id);
        if let Some(label) = name {
            self.from_node_name = label.clone();
            self.from_search = label;
        }
        self.show_from_search = false;
        self.from_suggestions.clear();
        self.from_selected_index = 0;
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
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        event_bus: &EventBus,
        search_engine: Option<&mut SearchEngine>,
        storage: Option<&Storage>,
    ) -> bool {
        if !self.is_open {
            return false;
        }

        let mut start_trail = false;
        let mut should_close = false;
        let mut search_engine = search_engine;

        let mut from_input_rect = None;
        let mut to_input_rect = None;

        egui::Window::new("Custom Trail")
            .order(egui::Order::Tooltip)
            .resizable(true)
            .default_width(450.0)
            .show(ctx, |ui| {
                ui.heading("Configure Trail");

                // Close button in header
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(crate::theme::small_icon_button(ph::X))
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
                            from_input_rect = Some(response.rect);
                            if response.changed() {
                                let query = self.from_search.clone();
                                update_suggestions(
                                    &query,
                                    &mut self.last_from_query,
                                    &mut self.from_suggestions,
                                    &mut self.from_selected_index,
                                    &mut self.show_from_search,
                                    search_engine.as_deref_mut(),
                                    storage,
                                );
                            }
                            if response.has_focus() {
                                handle_search_keys(
                                    ui,
                                    &mut self.from_selected_index,
                                    &mut self.show_from_search,
                                    self.from_suggestions.len(),
                                );
                                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    if let Some(match_) = self
                                        .from_suggestions
                                        .get(self.from_selected_index)
                                        .cloned()
                                    {
                                        self.select_from_match(match_);
                                    }
                                }
                            }
                            if self.from_node.is_some()
                                && ui.button(ph::X).on_hover_text("Clear").clicked()
                            {
                                self.from_node = None;
                                self.from_node_name.clear();
                                self.from_search.clear();
                                self.from_suggestions.clear();
                                self.show_from_search = false;
                            }
                        });
                        ui.end_row();

                        ui.label("To (optional):");
                        ui.horizontal(|ui| {
                            let response = ui.text_edit_singleline(&mut self.to_search);
                            to_input_rect = Some(response.rect);
                            if response.changed() {
                                let query = self.to_search.clone();
                                update_suggestions(
                                    &query,
                                    &mut self.last_to_query,
                                    &mut self.to_suggestions,
                                    &mut self.to_selected_index,
                                    &mut self.show_to_search,
                                    search_engine.as_deref_mut(),
                                    storage,
                                );
                            }
                            if response.has_focus() {
                                handle_search_keys(
                                    ui,
                                    &mut self.to_selected_index,
                                    &mut self.show_to_search,
                                    self.to_suggestions.len(),
                                );
                                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    if let Some(match_) = self
                                        .to_suggestions
                                        .get(self.to_selected_index)
                                        .cloned()
                                    {
                                        self.select_to_match(match_);
                                    }
                                }
                            }
                            if self.to_node.is_some()
                                && ui.button(ph::X).on_hover_text("Clear").clicked()
                            {
                                self.to_node = None;
                                self.to_node_name.clear();
                                self.to_search.clear();
                                self.to_suggestions.clear();
                                self.show_to_search = false;
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
                    let outgoing_label = format!("{} Outgoing", ph::ARROW_RIGHT);
                    let incoming_label = format!("{} Incoming", ph::ARROW_LEFT);
                    let both_label = format!("{} Both", ph::ARROWS_LEFT_RIGHT);
                    ui.selectable_value(
                        &mut self.direction,
                        TrailDirection::Outgoing,
                        outgoing_label,
                    );
                    ui.selectable_value(
                        &mut self.direction,
                        TrailDirection::Incoming,
                        incoming_label,
                    );
                    ui.selectable_value(&mut self.direction, TrailDirection::Both, both_label);
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

        if let Some(rect) = from_input_rect {
            if self.show_from_search && !self.from_suggestions.is_empty() {
                let selected = render_search_dropdown(
                    ctx,
                    rect,
                    &self.from_suggestions,
                    &mut self.from_selected_index,
                    "custom_trail_from_dropdown",
                );
                if let Some(match_) = selected {
                    self.select_from_match(match_);
                }
            }
        }

        if let Some(rect) = to_input_rect {
            if self.show_to_search && !self.to_suggestions.is_empty() {
                let selected = render_search_dropdown(
                    ctx,
                    rect,
                    &self.to_suggestions,
                    &mut self.to_selected_index,
                    "custom_trail_to_dropdown",
                );
                if let Some(match_) = selected {
                    self.select_to_match(match_);
                }
            }
        }

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

    fn select_from_match(&mut self, match_: SearchMatch) {
        self.from_node = Some(match_.node_id);
        self.from_node_name = match_.name.clone();
        self.from_search = match_.name;
        self.from_suggestions.clear();
        self.show_from_search = false;
        self.from_selected_index = 0;
    }

    fn select_to_match(&mut self, match_: SearchMatch) {
        self.to_node = Some(match_.node_id);
        self.to_node_name = match_.name.clone();
        self.to_search = match_.name;
        self.to_suggestions.clear();
        self.show_to_search = false;
        self.to_selected_index = 0;
    }
}

fn update_suggestions(
    query: &str,
    last_query: &mut String,
    suggestions: &mut Vec<SearchMatch>,
    selected_index: &mut usize,
    show_dropdown: &mut bool,
    search_engine: Option<&mut SearchEngine>,
    storage: Option<&Storage>,
) {
    if query.len() < 2 {
        suggestions.clear();
        *selected_index = 0;
        *show_dropdown = false;
        return;
    }

    let Some(engine) = search_engine else {
        suggestions.clear();
        *show_dropdown = false;
        return;
    };

    if query == last_query {
        *show_dropdown = !suggestions.is_empty();
        return;
    }

    *last_query = query.to_string();
    let ids = engine.search_symbol(query);
    *suggestions = build_search_matches(ids, storage);
    *selected_index = 0;
    *show_dropdown = !suggestions.is_empty();
}

fn handle_search_keys(
    ui: &egui::Ui,
    selected_index: &mut usize,
    show_dropdown: &mut bool,
    len: usize,
) {
    if len == 0 {
        return;
    }
    if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
        *selected_index = (*selected_index + 1) % len;
    }
    if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
        if *selected_index == 0 {
            *selected_index = len - 1;
        } else {
            *selected_index -= 1;
        }
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        *show_dropdown = false;
    }
}

fn render_search_dropdown(
    ctx: &egui::Context,
    input_rect: egui::Rect,
    suggestions: &[SearchMatch],
    selected_index: &mut usize,
    area_id: &str,
) -> Option<SearchMatch> {
    let mut selected_match = None;

    egui::Area::new(egui::Id::new(area_id))
        .fixed_pos(input_rect.left_bottom())
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(input_rect.width().max(360.0));
                ui.set_max_height(260.0);

                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (idx, suggestion) in suggestions.iter().enumerate() {
                        let is_selected = idx == *selected_index;
                        let row_response = ui
                            .push_id(idx, |ui| {
                                let available_width = ui.available_width();
                                let (rect, response) = ui.allocate_at_least(
                                    egui::vec2(available_width, 22.0),
                                    egui::Sense::click(),
                                );

                                let bg_color = if response.hovered() || is_selected {
                                    ui.visuals().widgets.hovered.bg_fill
                                } else {
                                    egui::Color32::TRANSPARENT
                                };
                                ui.painter().rect_filled(rect, 2.0, bg_color);

                                let mut content_rect = rect;
                                content_rect.min.x += 4.0;
                                content_rect.max.x -= 4.0;
                                let mut child_ui =
                                    ui.new_child(egui::UiBuilder::new().max_rect(content_rect));
                                child_ui.horizontal(|ui| {
                                    let kind_color = ui.visuals().selection.bg_fill;
                                    badge(ui, &suggestion.kind, kind_color);
                                    ui.label(egui::RichText::new(&suggestion.name).strong());
                                    if suggestion.qualified_name != suggestion.name {
                                        ui.label(
                                            egui::RichText::new(&suggestion.qualified_name)
                                                .small()
                                                .weak(),
                                        );
                                    }
                                });

                                response
                            })
                            .inner;

                        if row_response.clicked() {
                            selected_match = Some(suggestion.clone());
                        }

                        if row_response.hovered() {
                            *selected_index = idx;
                        }
                    }
                });
            });
        });

    selected_match
}

fn node_kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::MODULE => "mod",
        NodeKind::NAMESPACE => "ns",
        NodeKind::PACKAGE => "pkg",
        NodeKind::FILE => "file",
        NodeKind::STRUCT => "struct",
        NodeKind::CLASS => "class",
        NodeKind::INTERFACE => "iface",
        NodeKind::ANNOTATION => "anno",
        NodeKind::UNION => "union",
        NodeKind::ENUM => "enum",
        NodeKind::TYPEDEF => "typedef",
        NodeKind::TYPE_PARAMETER => "typeparam",
        NodeKind::BUILTIN_TYPE => "builtin",
        NodeKind::FUNCTION => "fn",
        NodeKind::METHOD => "method",
        NodeKind::MACRO => "macro",
        NodeKind::GLOBAL_VARIABLE => "gvar",
        NodeKind::FIELD => "field",
        NodeKind::VARIABLE => "var",
        NodeKind::CONSTANT => "const",
        NodeKind::ENUM_CONSTANT => "enumconst",
        NodeKind::UNKNOWN => "sym",
    }
}

fn build_search_matches(ids: Vec<NodeId>, storage: Option<&Storage>) -> Vec<SearchMatch> {
    ids.into_iter()
        .take(10)
        .map(|id| {
            let mut name = id.0.to_string();
            let mut kind_str = "symbol".to_string();
            let mut file_path = None;
            let mut line = None;

            if let Some(storage) = storage {
                if let Ok(Some(node)) = storage.get_node(id) {
                    name = node
                        .qualified_name
                        .clone()
                        .unwrap_or_else(|| node.serialized_name.clone());
                    kind_str = node_kind_label(node.kind).to_string();
                }
                if let Ok(occs) = storage.get_occurrences_for_node(id)
                    && let Some(occ) = occs.first()
                    && let Ok(Some(file_node)) = storage.get_node(occ.location.file_node_id)
                {
                    file_path = Some(file_node.serialized_name);
                    line = Some(occ.location.start_line);
                }
            }

            SearchMatch {
                node_id: id,
                name: name.clone(),
                qualified_name: name,
                kind: kind_str,
                file_path,
                line,
                score: 1.0,
            }
        })
        .collect()
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
