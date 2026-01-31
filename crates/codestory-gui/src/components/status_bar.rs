use crate::theme::{self, badge};
use eframe::egui;
use egui_phosphor::regular as ph;
use sysinfo::{Pid, ProcessesToUpdate, System};

pub struct StatusBar {
    system: System,
    pid: Pid,
    last_update: std::time::Instant,
    memory_usage_mb: u64,
}

impl StatusBar {
    pub fn new() -> Self {
        let mut system = System::new();
        let pid = Pid::from_u32(std::process::id());
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);

        Self {
            system,
            pid,
            last_update: std::time::Instant::now(),
            memory_usage_mb: 0,
        }
    }

    pub fn update_stats(&mut self) {
        if self.last_update.elapsed().as_secs() >= 1 {
            self.system
                .refresh_processes(ProcessesToUpdate::Some(&[self.pid]), true);
            if let Some(process) = self.system.process(self.pid) {
                self.memory_usage_mb = process.memory() / 1024 / 1024;
            }
            self.last_update = std::time::Instant::now();
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        status_message: &str,
        node_count: i64,
        error_count: usize,
        is_indexing: bool,
    ) -> (bool, bool) {
        // (start_indexing, toggle_error_panel)
        self.update_stats();
        let mut start_indexing = false;
        let mut toggle_errors = false;

        ui.horizontal(|ui| {
            if is_indexing {
                ui.add(egui::Spinner::new());
                ui.label(egui::RichText::new("Indexing...").color(ui.visuals().warn_fg_color));
            } else {
                badge(ui, "Ready", egui::Color32::LIGHT_GREEN);
            }

            ui.separator();
            ui.label(egui::RichText::new(status_message).color(ui.visuals().text_color()));

            if error_count > 0 {
                ui.separator();
                let error_text = format!("{} {} Errors", ph::WARNING_CIRCLE, error_count);
                if ui
                    .button(egui::RichText::new(error_text).color(ui.visuals().error_fg_color))
                    .clicked()
                {
                    toggle_errors = true;
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !is_indexing {
                    if ui
                        .add(theme::primary_button(ui, "Index Workspace"))
                        .clicked()
                    {
                        start_indexing = true;
                    }
                    ui.separator();
                }
                badge(
                    ui,
                    &format!("{} MB", self.memory_usage_mb),
                    ui.visuals().window_fill,
                );
                ui.separator();
                badge(
                    ui,
                    &format!("{} nodes", node_count),
                    ui.visuals().selection.bg_fill,
                );
            });
        });

        (start_indexing, toggle_errors)
    }
}
