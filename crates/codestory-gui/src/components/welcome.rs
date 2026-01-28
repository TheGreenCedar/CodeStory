use crate::components::file_dialog::{FileDialogManager, FileDialogPresets};
use crate::theme::{self, spacing};
use eframe::egui;
use std::path::PathBuf;

pub struct WelcomeScreen {
    pub recent_projects: Vec<PathBuf>,
}

impl WelcomeScreen {
    pub fn new() -> Self {
        Self {
            recent_projects: Vec::new(),
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        file_dialog: &mut FileDialogManager,
        settings: &crate::settings::AppSettings,
    ) -> Option<WelcomeAction> {
        let mut action = None;

        ui.vertical_centered(|ui| {
            ui.add_space(50.0);
            ui.heading(
                egui::RichText::new("CodeStory")
                    .size(40.0)
                    .size(40.0)
                    .strong()
                    .color(ui.visuals().selection.bg_fill),
            );
            ui.label(
                egui::RichText::new("Modern Source Code Explorer").color(ui.visuals().text_color()),
            );
            ui.add_space(20.0);
            ui.add(theme::large_icon_button("ℹ"))
                .on_hover_text("About CodeStory");
            ui.add_space(10.0);

            if ui
                .add(
                    theme::primary_button(ui, "📂 Open Project Folder")
                        .min_size(egui::vec2(200.0, 40.0)),
                )
                .clicked()
            {
                FileDialogPresets::open_project(file_dialog);
                file_dialog.open_directory(
                    "Open CodeStory Project Folder",
                    "open_project",
                    &settings.file_dialog,
                );
            }

            ui.add_space(spacing::SECTION_SPACING);

            theme::card(ui, |ui| {
                ui.heading("Recent Projects");
                ui.add_space(spacing::ITEM_SPACING);

                if self.recent_projects.is_empty() {
                    theme::empty_state(
                        ui,
                        "📂",
                        "No Recent Projects",
                        "Open a project folder to get started",
                    );
                } else {
                    for path in &self.recent_projects {
                        let display_name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.to_string_lossy().to_string());

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("📁").color(ui.visuals().selection.bg_fill),
                            );
                            if ui.link(&display_name).clicked() {
                                action = Some(WelcomeAction::OpenRecent(path.clone()));
                            }
                            ui.label(
                                egui::RichText::new(path.to_string_lossy())
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                            );
                        });
                    }
                }
            });
        });

        action
    }
}

pub enum WelcomeAction {
    OpenRecent(PathBuf),
}
