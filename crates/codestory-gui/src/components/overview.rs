use crate::theme::spacing;
use codestory_storage::StorageStats;
use eframe::egui;

pub struct ProjectOverview {
    pub project_name: String,
    pub stats: Option<StorageStats>,
}

impl ProjectOverview {
    pub fn new(name: String) -> Self {
        Self {
            project_name: name,
            stats: None,
        }
    }

    pub fn set_stats(&mut self, stats: StorageStats) {
        self.stats = Some(stats);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(spacing::SECTION_SPACING);

            ui.heading(
                egui::RichText::new(&self.project_name)
                    .size(32.0)
                    .strong()
                    .color(ui.visuals().selection.bg_fill),
            );
            ui.label(egui::RichText::new("Project Overview").color(ui.visuals().text_color()));

            ui.add_space(spacing::SECTION_SPACING);

            if let Some(stats) = &self.stats {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 20.0;

                    self.stat_card(ui, "📄", "Files", stats.file_count);
                    self.stat_card(ui, "🧬", "Symbols", stats.node_count);
                    self.stat_card(ui, "🔗", "Edges", stats.edge_count);
                    self.stat_card(ui, "⚠️", "Errors", stats.error_count);
                });
            } else {
                ui.spinner();
            }

            ui.add_space(spacing::SECTION_SPACING);

            ui.group(|ui| {
                ui.set_width(400.0);
                ui.heading("Quick Actions");
                ui.add_space(spacing::ITEM_SPACING);

                if ui.button("🔍 Search Symbols (Ctrl+P)").clicked() {
                    // This could trigger an event
                }

                if ui.button("🗂 Browse Symbol Hierarchy").clicked() {
                    // This could trigger an event
                }

                if ui.button("🛠 Project Settings").clicked() {
                    // This could trigger an event
                }
            });
        });
    }

    fn stat_card(&self, ui: &mut egui::Ui, icon: &str, label: &str, count: i64) {
        ui.group(|ui| {
            ui.set_min_width(120.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(icon).size(24.0));
                ui.label(egui::RichText::new(count.to_string()).size(20.0).strong());
                ui.label(
                    egui::RichText::new(label)
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            });
        });
    }
}
