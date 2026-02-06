use crate::theme::{self, empty_state, labeled_separator, spacing};
use codestory_core::{Edge, Node};
use eframe::egui;
use egui_phosphor::regular as ph;

pub struct DetailPanel {
    pub selected_node: Option<Node>,
    pub edges: Vec<Edge>,
}

impl DetailPanel {
    pub fn new() -> Self {
        Self {
            selected_node: None,
            edges: Vec::new(),
        }
    }

    pub fn set_data(&mut self, node: Node, edges: Vec<Edge>) {
        self.selected_node = Some(node);
        self.edges = edges;
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        bookmark_panel: &crate::components::bookmark_panel::BookmarkPanel,
    ) {
        ui.vertical(|ui| {
            if let Some(node) = &self.selected_node {
                theme::card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading(
                            egui::RichText::new(&node.serialized_name)
                                .color(ui.visuals().selection.bg_fill),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button(ph::BOOKMARK)
                                .on_hover_text("Quick Bookmark")
                                .clicked()
                            {
                                bookmark_panel.add_bookmark_to_default(node.id);
                            }
                        });
                    });
                    ui.add_space(spacing::ITEM_SPACING);

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Kind:").color(ui.visuals().text_color()));
                        theme::badge(
                            ui,
                            &format!("{:?}", node.kind),
                            ui.visuals().selection.bg_fill,
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("ID:").color(ui.visuals().text_color()));
                        ui.label(
                            egui::RichText::new(format!("{}", node.id.0))
                                .color(ui.visuals().weak_text_color()),
                        );
                    });
                });

                ui.add_space(spacing::SECTION_SPACING);
                labeled_separator(ui, "Relationships");
                ui.add_space(spacing::ITEM_SPACING);

                egui::ScrollArea::vertical()
                    .id_salt("detail_edges")
                    .show(ui, |ui| {
                        if self.edges.is_empty() {
                            theme::info_box(ui, "No relationships found for this node.");
                        } else {
                            for edge in &self.edges {
                                let (eff_source, eff_target) = edge.effective_endpoints();
                                // Determine direction relative to center
                                let is_source = eff_source == node.id;
                                let (direction, direction_color) = if is_source {
                                    (ph::ARROW_RIGHT, egui::Color32::LIGHT_GREEN)
                                } else {
                                    (ph::ARROW_LEFT, ui.visuals().selection.bg_fill)
                                };
                                let other_id = if is_source { eff_target } else { eff_source };

                                ui.horizontal(|ui| {
                                    theme::badge(
                                        ui,
                                        &format!("{:?}", edge.kind),
                                        ui.visuals().window_fill,
                                    );
                                    ui.label(egui::RichText::new(direction).color(direction_color));
                                    ui.label(format!("Node {}", other_id.0));
                                });
                            }
                        }
                    });
            } else {
                empty_state(
                    ui,
                    ph::INFO,
                    "No Selection",
                    "Select a node to view details",
                );
            }
        });
    }
}
