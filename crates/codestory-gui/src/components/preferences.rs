use crate::settings::{AppSettings, ThemeMode};
use eframe::egui;

pub struct PreferencesDialog {
    pub open: bool,
    temp_settings: AppSettings,
}

impl PreferencesDialog {
    pub fn new(current_settings: &AppSettings) -> Self {
        Self {
            open: false,
            temp_settings: current_settings.clone(),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, settings: &mut AppSettings) {
        let mut open = self.open;
        if !open {
            return;
        }

        let mut should_close = false;
        egui::Window::new("Preferences")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.heading("General");
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label("Theme:");
                        ui.radio_value(&mut self.temp_settings.theme, ThemeMode::Latte, "Latte");
                        ui.radio_value(&mut self.temp_settings.theme, ThemeMode::Frappe, "Frappé");
                        ui.radio_value(
                            &mut self.temp_settings.theme,
                            ThemeMode::Macchiato,
                            "Macchiato",
                        );
                        ui.radio_value(&mut self.temp_settings.theme, ThemeMode::Mocha, "Mocha");
                    });

                    ui.add(
                        egui::Slider::new(&mut self.temp_settings.ui_scale, 0.5..=2.0)
                            .text("UI Scale"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.temp_settings.font_size, 8.0..=24.0)
                            .text("Font Size"),
                    );
                    ui.checkbox(&mut self.temp_settings.show_tooltips, "Show Tooltips");
                    ui.checkbox(
                        &mut self.temp_settings.auto_open_last_project,
                        "Open last project on launch",
                    );

                    ui.separator();
                    ui.label("Appearance:");
                    ui.checkbox(&mut self.temp_settings.show_icons, "Show Icons");
                    ui.checkbox(&mut self.temp_settings.compact_mode, "Compact Mode");
                    ui.add(
                        egui::Slider::new(&mut self.temp_settings.animation_speed, 0.0..=2.0)
                            .text("Animation Speed"),
                    );
                });

                ui.add_space(10.0);
                ui.heading("Notifications");
                ui.group(|ui| {
                    ui.checkbox(
                        &mut self.temp_settings.notifications.enabled,
                        "Enable Toast Notifications",
                    );

                    if self.temp_settings.notifications.enabled {
                        ui.horizontal(|ui| {
                            ui.label("Position:");
                            egui::ComboBox::from_id_salt("notif_pos")
                                .selected_text(format!(
                                    "{:?}",
                                    self.temp_settings.notifications.position
                                ))
                                .show_ui(ui, |ui| {
                                    use crate::settings::NotificationPosition;
                                    ui.selectable_value(
                                        &mut self.temp_settings.notifications.position,
                                        NotificationPosition::TopRight,
                                        "Top Right",
                                    );
                                    ui.selectable_value(
                                        &mut self.temp_settings.notifications.position,
                                        NotificationPosition::TopLeft,
                                        "Top Left",
                                    );
                                    ui.selectable_value(
                                        &mut self.temp_settings.notifications.position,
                                        NotificationPosition::BottomRight,
                                        "Bottom Right",
                                    );
                                    ui.selectable_value(
                                        &mut self.temp_settings.notifications.position,
                                        NotificationPosition::BottomLeft,
                                        "Bottom Left",
                                    );
                                });
                        });

                        ui.separator();
                        ui.label("Categories:");
                        ui.checkbox(
                            &mut self.temp_settings.notifications.show_indexing_progress,
                            "Indexing Progress",
                        );
                        ui.checkbox(
                            &mut self.temp_settings.notifications.show_search_results,
                            "Search Results",
                        );
                        ui.checkbox(
                            &mut self.temp_settings.notifications.show_file_operations,
                            "File Operations",
                        );
                    }
                });

                ui.add_space(10.0);
                ui.heading("File Dialogs");
                ui.group(|ui| {
                    ui.checkbox(
                        &mut self.temp_settings.file_dialog.use_custom_dialogs,
                        "Use Integrated File Dialogs",
                    );
                    ui.label(
                        egui::RichText::new("(Disable to use native OS file dialogs)")
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );

                    if self.temp_settings.file_dialog.use_custom_dialogs {
                        ui.checkbox(
                            &mut self.temp_settings.file_dialog.show_hidden_files,
                            "Show Hidden Files",
                        );

                        ui.horizontal(|ui| {
                            ui.label("Default Size:");
                            ui.add(
                                egui::DragValue::new(
                                    &mut self.temp_settings.file_dialog.default_width,
                                )
                                .range(400.0..=1200.0)
                                .suffix("px"),
                            );
                            ui.label("×");
                            ui.add(
                                egui::DragValue::new(
                                    &mut self.temp_settings.file_dialog.default_height,
                                )
                                .range(300.0..=900.0)
                                .suffix("px"),
                            );
                        });
                    }
                });

                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        *settings = self.temp_settings.clone();
                        settings.save();
                        self.apply_theme(ctx, settings);
                    }
                    if ui.button("Close").clicked() {
                        should_close = true;
                    }
                });
            });

        if should_close {
            open = false;
        }
        self.open = open;
    }

    pub fn sync_with_current(&mut self, settings: &AppSettings) {
        self.temp_settings = settings.clone();
    }

    fn apply_theme(&self, ctx: &egui::Context, settings: &AppSettings) {
        ctx.set_pixels_per_point(settings.ui_scale);

        // Convert settings to Theme and apply
        let flavor = match settings.theme {
            crate::settings::ThemeMode::Latte => catppuccin_egui::LATTE,
            crate::settings::ThemeMode::Frappe => catppuccin_egui::FRAPPE,
            crate::settings::ThemeMode::Macchiato => catppuccin_egui::MACCHIATO,
            crate::settings::ThemeMode::Mocha => catppuccin_egui::MOCHA,
        };

        let mut theme = crate::theme::Theme::new(flavor);

        theme.font_size_base = settings.font_size;
        theme.font_size_small = settings.font_size * 0.85;
        theme.font_size_heading = settings.font_size * 1.25;
        theme.show_icons = settings.show_icons;
        theme.compact_mode = settings.compact_mode;
        theme.animation_speed = settings.animation_speed;

        theme.apply(ctx);
    }
}
