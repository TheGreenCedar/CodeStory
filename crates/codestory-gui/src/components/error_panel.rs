use codestory_core::{ErrorFilter, NodeId};
use codestory_events::{Event, EventBus};
use codestory_storage::Storage;
use eframe::egui;
use std::collections::HashMap;

pub struct ErrorPanel {
    pub fatal_only: bool,
    pub indexed_only: bool,
    pub file_names: HashMap<NodeId, String>,
    pub event_bus: EventBus,
}

impl ErrorPanel {
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            fatal_only: false,
            indexed_only: false,
            file_names: HashMap::new(),
            event_bus,
        }
    }

    pub fn set_file_name(&mut self, id: NodeId, name: String) {
        self.file_names.insert(id, name);
    }

    pub fn filter_by_file(&mut self, id: Option<NodeId>) {
        if id.is_some() {
            self.indexed_only = true;
        }
    }

    pub fn clear_filters(&mut self) {
        self.fatal_only = false;
        self.indexed_only = false;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, storage: &Option<Storage>) {
        let Some(storage) = storage else {
            ui.centered_and_justified(|ui| {
                ui.label("No project loaded. Open a project to see errors.");
            });
            return;
        };

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.fatal_only, "Fatal Only");
            ui.checkbox(&mut self.indexed_only, "Current File Only");

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Clear All").clicked() {
                    let _ = storage.clear_errors();
                }
            });
        });
        ui.separator();

        let filter = ErrorFilter {
            fatal_only: self.fatal_only,
            indexed_only: self.indexed_only,
        };

        match storage.get_errors(Some(&filter)) {
            Ok(errors) => {
                if errors.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(20.0);
                        ui.label(
                            egui::RichText::new("✓ No errors found.")
                                .color(egui::Color32::LIGHT_GREEN),
                        );
                    });
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for err in errors {
                            let color = if err.is_fatal {
                                ui.visuals().error_fg_color
                            } else {
                                ui.visuals().warn_fg_color
                            };

                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!("[{:?}]", err.index_step))
                                            .color(color)
                                            .monospace(),
                                    );

                                    if let Some(file_id) = err.file_id {
                                        let name = self
                                            .file_names
                                            .get(&file_id)
                                            .cloned()
                                            .unwrap_or_else(|| format!("File #{}", file_id.0));
                                        let loc = format!("{}:{}", name, err.line.unwrap_or(0));
                                        if ui.link(loc).clicked() {
                                            self.event_bus.publish(Event::ErrorNavigate {
                                                file_id,
                                                line: err.line.unwrap_or(1),
                                            });
                                        }
                                    }
                                });

                                ui.label(
                                    egui::RichText::new(&err.message)
                                        .color(ui.visuals().text_color()),
                                );
                            });
                            ui.add_space(4.0);
                        }
                    });
                }
            }
            Err(e) => {
                ui.label(format!("Error fetching diagnostics: {}", e));
            }
        }
    }
}
