use crate::theme::{badge, empty_state};
use codestory_core::{NodeId, Occurrence};
use eframe::egui;
use std::collections::HashMap;

pub struct ReferenceList {
    occurrences: Vec<Occurrence>,
}

impl ReferenceList {
    pub fn new() -> Self {
        Self {
            occurrences: Vec::new(),
        }
    }

    pub fn set_data(&mut self, occurrences: Vec<Occurrence>) {
        self.occurrences = occurrences;
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        node_names: &HashMap<NodeId, String>,
    ) -> Option<Occurrence> {
        let mut selected = None;
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("References").color(ui.visuals().selection.bg_fill));
                badge(
                    ui,
                    &format!("{}", self.occurrences.len()),
                    ui.visuals().selection.bg_fill,
                );
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("reference_list_scroll")
                .max_height(300.0)
                .show(ui, |ui| {
                    if self.occurrences.is_empty() {
                        empty_state(
                            ui,
                            "📍",
                            "No References",
                            "Select a symbol to see references",
                        );
                    } else {
                        for occ in &self.occurrences {
                            let file_id = occ.location.file_node_id;
                            let file_name = node_names
                                .get(&file_id)
                                .map(|s| s.as_str())
                                .unwrap_or("Unknown File");

                            // Shorten file name for display if it's a path
                            let short_name = std::path::Path::new(file_name)
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or(file_name);

                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("📍").color(ui.visuals().selection.bg_fill),
                                );
                                let label = format!("{}:{}", short_name, occ.location.start_line);
                                if ui.link(label).on_hover_text(file_name).clicked() {
                                    selected = Some(occ.clone());
                                }
                            });
                        }
                    }
                });
        });
        selected
    }
}
