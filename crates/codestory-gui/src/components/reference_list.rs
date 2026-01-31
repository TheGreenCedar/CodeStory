use crate::theme::{badge, empty_state};
use codestory_core::{NodeId, Occurrence};
use eframe::egui;
use egui_phosphor::regular as ph;
use std::collections::HashMap;

pub struct ReferenceList {
    occurrences: Vec<Occurrence>,
    filtered: Vec<Occurrence>,
    active_file: Option<NodeId>,
    show_local_only: bool,
    current_index: Option<usize>,
}

impl ReferenceList {
    pub fn new() -> Self {
        Self {
            occurrences: Vec::new(),
            filtered: Vec::new(),
            active_file: None,
            show_local_only: false,
            current_index: None,
        }
    }

    pub fn set_data(&mut self, occurrences: Vec<Occurrence>, active_file: Option<NodeId>) {
        self.occurrences = occurrences;
        self.active_file = active_file;
        self.refresh_filtered();
    }

    pub fn set_active_file(&mut self, active_file: Option<NodeId>) {
        self.active_file = active_file;
        self.refresh_filtered();
    }

    pub fn occurrences_for_file(&self, file_id: NodeId) -> Vec<Occurrence> {
        self.occurrences
            .iter()
            .filter(|occ| occ.location.file_node_id == file_id)
            .cloned()
            .collect()
    }

    pub fn filtered_len(&self) -> usize {
        self.filtered.len()
    }

    pub fn position_label(&self) -> String {
        if self.filtered.is_empty() {
            "0/0".to_string()
        } else {
            let idx = self.current_index.unwrap_or(0) + 1;
            format!("{}/{}", idx, self.filtered.len())
        }
    }

    pub fn next_occurrence(&mut self) -> Option<Occurrence> {
        if self.filtered.is_empty() {
            return None;
        }
        let next_index = match self.current_index {
            Some(idx) => (idx + 1) % self.filtered.len(),
            None => 0,
        };
        self.current_index = Some(next_index);
        self.filtered.get(next_index).cloned()
    }

    pub fn prev_occurrence(&mut self) -> Option<Occurrence> {
        if self.filtered.is_empty() {
            return None;
        }
        let prev_index = match self.current_index {
            Some(idx) => idx.checked_sub(1).unwrap_or(self.filtered.len() - 1),
            None => 0,
        };
        self.current_index = Some(prev_index);
        self.filtered.get(prev_index).cloned()
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
                    &format!("{}", self.filtered.len()),
                    ui.visuals().selection.bg_fill,
                );

                ui.add_space(8.0);
                if ui
                    .selectable_value(&mut self.show_local_only, false, "All")
                    .clicked()
                {
                    self.refresh_filtered();
                }
                if ui
                    .selectable_value(&mut self.show_local_only, true, "Local")
                    .clicked()
                {
                    self.refresh_filtered();
                }

                ui.add_space(8.0);
                if ui.button(ph::CARET_UP).on_hover_text("Previous").clicked() {
                    selected = self.prev_occurrence();
                }
                if ui.button(ph::CARET_DOWN).on_hover_text("Next").clicked() {
                    selected = self.next_occurrence();
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("reference_list_scroll")
                .max_height(300.0)
                .show(ui, |ui| {
                    if self.filtered.is_empty() {
                        empty_state(
                            ui,
                            ph::MAP_PIN,
                            "No References",
                            "Select a symbol to see references",
                        );
                    } else {
                        for (idx, occ) in self.filtered.iter().enumerate() {
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

                            let bg = if Some(idx) == self.current_index {
                                ui.visuals().selection.bg_fill.linear_multiply(0.2)
                            } else {
                                egui::Color32::TRANSPARENT
                            };
                            egui::Frame::NONE.fill(bg).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(ph::MAP_PIN)
                                            .color(ui.visuals().selection.bg_fill),
                                    );
                                    let label =
                                        format!("{}:{}", short_name, occ.location.start_line);
                                    if ui.link(label).on_hover_text(file_name).clicked() {
                                        self.current_index = Some(idx);
                                        selected = Some(occ.clone());
                                    }
                                });
                            });
                        }
                    }
                });
        });
        selected
    }

    fn refresh_filtered(&mut self) {
        self.filtered = if self.show_local_only {
            if let Some(file_id) = self.active_file {
                self.occurrences
                    .iter()
                    .filter(|occ| occ.location.file_node_id == file_id)
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            self.occurrences.clone()
        };
        if self.filtered.is_empty() {
            self.current_index = None;
        } else if self.current_index.is_none()
            || self.current_index.unwrap_or(0) >= self.filtered.len()
        {
            self.current_index = Some(0);
        }
    }
}
