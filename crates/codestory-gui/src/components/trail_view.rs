//! Trail View Component
//!
//! Displays a depth-limited subgraph for focused exploration.

use crate::theme::{self, badge};
use codestory_core::{EdgeKind, NodeId, NodeKind, TrailConfig, TrailDirection, TrailMode};
use codestory_events::{Event, EventBus};
use eframe::egui;

/// Trail view mode configuration
#[derive(Debug, Clone)]
pub struct TrailViewConfig {
    pub root_id: Option<NodeId>,
    pub mode: TrailMode,
    pub target_id: Option<NodeId>,
    pub depth: u32,
    pub direction: TrailDirection,
    pub edge_filter: Vec<EdgeKind>,
    pub node_filter: Vec<NodeKind>,
    pub max_nodes: usize,
    pub use_radial_layout: bool,
}

impl Default for TrailViewConfig {
    fn default() -> Self {
        Self {
            root_id: None,
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: 2,
            direction: TrailDirection::Both,
            edge_filter: vec![],
            node_filter: vec![],
            max_nodes: 500,
            use_radial_layout: false,
        }
    }
}

impl TrailViewConfig {
    pub fn to_trail_config(&self) -> Option<TrailConfig> {
        self.root_id.map(|root_id| TrailConfig {
            root_id,
            mode: self.mode,
            target_id: self.target_id,
            depth: self.depth,
            direction: self.direction,
            edge_filter: self.edge_filter.clone(),
            node_filter: self.node_filter.clone(),
            max_nodes: self.max_nodes,
        })
    }
}

/// Trail view control panel
pub struct TrailViewControls {
    pub config: TrailViewConfig,
    pub active: bool,
    event_bus: EventBus,
    edge_filter_popup: EdgeFilterPopup,
}

impl TrailViewControls {
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            config: TrailViewConfig::default(),
            active: false,
            event_bus,
            edge_filter_popup: EdgeFilterPopup::new(),
        }
    }

    /// Activate trail mode from a specific node
    pub fn activate(&mut self, root_id: NodeId) {
        self.activate_from_event(root_id);
        self.event_bus.publish(Event::TrailModeEnter { root_id });
    }

    /// Activate trail mode from an already-dispatched event
    pub fn activate_from_event(&mut self, root_id: NodeId) {
        self.config.root_id = Some(root_id);
        self.active = true;
    }

    /// Deactivate trail mode
    pub fn deactivate(&mut self) {
        self.active = false;
        self.config.root_id = None;
        self.event_bus.publish(Event::TrailModeExit);
    }

    /// Render the controls UI
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Mode toggle with badge
            let mode_text = if self.active { "Trail Mode" } else { "Normal" };
            if self.active {
                badge(ui, mode_text, ui.visuals().selection.bg_fill);
            } else if ui.selectable_label(false, mode_text).clicked() {
                // Do nothing - needs right-click to activate
            }

            if !self.active {
                ui.label(
                    egui::RichText::new("(Right-click a node to start trail)")
                        .color(ui.visuals().text_color()),
                );
                return;
            }

            ui.separator();

            // Depth slider
            ui.label("Depth:");
            if ui
                .add(egui::Slider::new(&mut self.config.depth, 1..=5))
                .changed()
            {
                self.notify_config_change();
            }

            ui.separator();

            // Direction selector
            ui.label("Direction:");
            egui::ComboBox::from_id_salt("trail_direction")
                .selected_text(match self.config.direction {
                    TrailDirection::Incoming => "Incoming",
                    TrailDirection::Outgoing => "Outgoing",
                    TrailDirection::Both => "Both",
                })
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(&mut self.config.direction, TrailDirection::Both, "Both")
                        .clicked()
                    {
                        self.notify_config_change();
                    }
                    if ui
                        .selectable_value(
                            &mut self.config.direction,
                            TrailDirection::Incoming,
                            "Incoming",
                        )
                        .clicked()
                    {
                        self.notify_config_change();
                    }
                    if ui
                        .selectable_value(
                            &mut self.config.direction,
                            TrailDirection::Outgoing,
                            "Outgoing",
                        )
                        .clicked()
                    {
                        self.notify_config_change();
                    }
                });

            ui.separator();

            // Edge type filter
            if ui
                .add(theme::secondary_button(ui, "Filter Edges..."))
                .clicked()
            {
                self.edge_filter_popup.show = true;
            }

            // Render popup if visible
            if self.edge_filter_popup.ui(ui) {
                // Edge filter changed, update config
                self.config.edge_filter = self.edge_filter_popup.selected.clone();
                self.notify_config_change();
            }

            ui.separator();

            // Exit button
            if ui.add(theme::danger_button(ui, "Exit Trail")).clicked() {
                self.deactivate();
            }

            ui.separator();

            // Layout toggle
            if ui
                .checkbox(&mut self.config.use_radial_layout, "Radial")
                .changed()
            {
                self.notify_config_change();
            }
        });
    }

    fn notify_config_change(&self) {
        self.event_bus.publish(Event::TrailConfigChange {
            depth: self.config.depth,
            direction: self.config.direction,
            edge_filter: self.config.edge_filter.clone(),
            mode: self.config.mode,
            target_id: self.config.target_id,
            node_filter: self.config.node_filter.clone(),
        });
    }
}

/// Edge type filter popup
pub struct EdgeFilterPopup {
    pub show: bool,
    pub selected: Vec<EdgeKind>,
}

impl EdgeFilterPopup {
    pub fn new() -> Self {
        Self {
            show: false,
            selected: vec![],
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;

        if !self.show {
            return false;
        }

        egui::Window::new("Filter Edge Types")
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let all_kinds = [
                    EdgeKind::MEMBER,
                    EdgeKind::CALL,
                    EdgeKind::USAGE,
                    EdgeKind::TYPE_USAGE,
                    EdgeKind::INHERITANCE,
                    EdgeKind::OVERRIDE,
                    EdgeKind::IMPORT,
                    EdgeKind::INCLUDE,
                ];

                for kind in all_kinds {
                    let mut is_selected = self.selected.contains(&kind);
                    if ui
                        .checkbox(&mut is_selected, format!("{:?}", kind))
                        .changed()
                    {
                        if is_selected {
                            self.selected.push(kind);
                        } else {
                            self.selected.retain(|k| *k != kind);
                        }
                        changed = true;
                    }
                }

                ui.separator();

                ui.horizontal(|ui| {
                    if ui.button("Select All").clicked() {
                        self.selected = all_kinds.to_vec();
                        changed = true;
                    }
                    if ui.button("Deselect All").clicked() {
                        self.selected.clear();
                        changed = true;
                    }
                    if ui.button("Close").clicked() {
                        self.show = false;
                    }
                });
            });

        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trail_config_conversion() {
        let config = TrailViewConfig {
            root_id: Some(NodeId(42)),
            depth: 3,
            ..Default::default()
        };

        let trail_config = config.to_trail_config().unwrap();
        assert_eq!(trail_config.root_id, NodeId(42));
        assert_eq!(trail_config.depth, 3);
    }
}
