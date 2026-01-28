//! CodeStory Theme and UI Polish
//!
//! Provides consistent styling, colors, and visual polish across the application.
//! Now powered by catppuccin-egui.

use eframe::egui::{self, Color32, Vec2};
use egui_dock::Style as DockStyle;

// Re-export specific colors if needed by legacy code, but mapped to empty structs or similar
// actually, for backward compatibility during refactor, I'll try to keep the module structure
// but implement it using visuals where possible, or just remove if I update all callsites.
// Plan says "Refactor helpers... to use ui.visuals()".
// I will keep spacing and radius.

/// Spacing constants
pub mod spacing {
    pub const PANEL_PADDING: f32 = 12.0;
    pub const PANEL_PADDING_I8: i8 = 12;
    pub const ITEM_SPACING: f32 = 8.0;
    pub const SECTION_SPACING: f32 = 16.0;
    pub const BUTTON_PADDING: f32 = 8.0;
    pub const ICON_SIZE: f32 = 16.0;
    pub const SMALL_ICON: f32 = 12.0;
    pub const LARGE_ICON: f32 = 24.0;
}

/// Border radius constants
pub mod radius {
    use eframe::egui::CornerRadius;

    pub const SMALL: CornerRadius = CornerRadius::same(2);
    pub const MEDIUM: CornerRadius = CornerRadius::same(4);
    pub const LARGE: CornerRadius = CornerRadius::same(8);
    pub const PILL: CornerRadius = CornerRadius::same(255);
}

/// Application theme configuration
#[derive(Debug, Clone)]
pub struct Theme {
    pub flavor: catppuccin_egui::Theme,

    /// Font customization fields
    pub font_size_base: f32,
    pub font_size_small: f32,
    pub font_size_heading: f32,
    pub show_icons: bool,
    pub compact_mode: bool,
    pub animation_speed: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            flavor: catppuccin_egui::MOCHA,
            font_size_base: 13.0,
            font_size_small: 11.0,
            font_size_heading: 16.0,
            show_icons: true,
            compact_mode: false,
            animation_speed: 1.0,
        }
    }
}

impl Theme {
    pub fn new(flavor: catppuccin_egui::Theme) -> Self {
        Self {
            flavor,
            ..Default::default()
        }
    }

    /// Apply theme to egui context
    pub fn apply(&self, ctx: &egui::Context) {
        catppuccin_egui::set_theme(ctx, self.flavor);
        self.setup_fonts(ctx);
    }

    fn setup_fonts(&self, ctx: &egui::Context) {
        // Load default fonts (custom fonts can be added here if available)
        let fonts = egui::FontDefinitions::default();
        ctx.set_fonts(fonts);

        // Set style
        let mut style = (*ctx.style()).clone();

        use egui::FontFamily::{Monospace, Proportional};
        use egui::FontId;
        use egui::TextStyle::{Body, Button, Heading, Small};

        style.text_styles = [
            (Heading, FontId::new(self.font_size_heading, Proportional)),
            (Body, FontId::new(self.font_size_base, Proportional)),
            (
                egui::TextStyle::Monospace,
                FontId::new(self.font_size_base, Monospace),
            ),
            (Button, FontId::new(self.font_size_base, Proportional)),
            (Small, FontId::new(self.font_size_small, Proportional)),
        ]
        .into();

        style.spacing.item_spacing = Vec2::new(spacing::ITEM_SPACING, spacing::ITEM_SPACING);
        style.spacing.button_padding =
            Vec2::new(spacing::BUTTON_PADDING, spacing::BUTTON_PADDING / 2.0);
        style.spacing.window_margin = egui::Margin::same(spacing::PANEL_PADDING as i8);

        // Interaction
        style.interaction.show_tooltips_only_when_still = false;

        ctx.set_style(style);
    }
}

// Helpers using current Context/Ui visuals

/// Helper to create styled buttons
pub fn primary_button(ui: &egui::Ui, text: &str) -> egui::Button<'static> {
    // Primary is usually stored in selection.bg_fill or similar in Catppuccin theme
    let color = ui.visuals().selection.bg_fill;
    let text_color = ui.visuals().strong_text_color();
    egui::Button::new(egui::RichText::new(text).color(text_color)).fill(color)
}

/// Helper to create styled secondary buttons
pub fn secondary_button(ui: &egui::Ui, text: &str) -> egui::Button<'static> {
    let color = ui.visuals().faint_bg_color;
    let text_color = ui.visuals().text_color();
    egui::Button::new(egui::RichText::new(text).color(text_color)).fill(color)
}

/// Helper to create icon buttons with standard sizing
pub fn icon_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).size(spacing::ICON_SIZE))
}

/// Helper to create small icon buttons
pub fn small_icon_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).size(spacing::SMALL_ICON))
}

/// Helper to create large icon buttons
pub fn large_icon_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).size(spacing::LARGE_ICON))
}

// / Helper for colors
pub fn to_egui_color(color: codestory_graph::Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

pub fn danger_button(ui: &egui::Ui, text: &str) -> egui::Button<'static> {
    let color = ui.visuals().error_fg_color;
    egui::Button::new(egui::RichText::new(text).color(ui.visuals().strong_text_color())).fill(color)
}

/// Create a styled separator with label
pub fn labeled_separator(ui: &mut egui::Ui, label: &str) {
    ui.horizontal(|ui| {
        ui.separator();
        ui.label(
            egui::RichText::new(label)
                .small()
                .color(ui.visuals().weak_text_color()),
        );
        ui.separator();
    });
}

/// Badge component for counts or status
pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    let frame = egui::Frame::default()
        .fill(color)
        .corner_radius(radius::PILL)
        .inner_margin(egui::Margin::symmetric(6, 2));

    frame.show(ui, |ui| {
        ui.label(
            egui::RichText::new(text)
                .small()
                .color(ui.visuals().strong_text_color()),
        );
    });
}

/// Card container with elevation effect - theme-aware
pub fn card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    let frame = egui::Frame::default()
        .fill(ui.visuals().window_fill) // Use window fill for cards on panel
        .corner_radius(radius::LARGE)
        .inner_margin(egui::Margin::same(spacing::PANEL_PADDING_I8))
        .stroke(ui.visuals().window_stroke);

    frame.show(ui, |ui| {
        add_contents(ui);
    });
}

/// Info box with icon
pub fn info_box(ui: &mut egui::Ui, message: &str) {
    let bg = ui.visuals().selection.bg_fill.gamma_multiply(0.2);
    let fg = ui.visuals().selection.bg_fill;

    let frame = egui::Frame::default()
        .fill(bg)
        .corner_radius(radius::MEDIUM)
        .inner_margin(egui::Margin::same(8));

    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("ℹ").color(fg));
            ui.label(message);
        });
    });
}

/// Warning box with icon
pub fn warning_box(ui: &mut egui::Ui, message: &str) {
    let fg = ui.visuals().warn_fg_color;
    let bg = fg.gamma_multiply(0.2);

    let frame = egui::Frame::default()
        .fill(bg)
        .corner_radius(radius::MEDIUM)
        .inner_margin(egui::Margin::same(8));

    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("⚠").color(fg));
            ui.label(message);
        });
    });
}

/// Error box with icon
pub fn error_box(ui: &mut egui::Ui, message: &str) {
    let fg = ui.visuals().error_fg_color;
    let bg = fg.gamma_multiply(0.2);

    let frame = egui::Frame::default()
        .fill(bg)
        .corner_radius(radius::MEDIUM)
        .inner_margin(egui::Margin::same(8));

    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("❌").color(fg));
            ui.label(message);
        });
    });
}

/// Progress indicator
pub fn progress_bar(ui: &mut egui::Ui, progress: f32, label: Option<&str>) {
    let desired_size = Vec2::new(ui.available_width(), 6.0);
    let (rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

    // Background
    ui.painter()
        .rect_filled(rect, radius::SMALL, ui.visuals().faint_bg_color);

    // Progress
    let progress_width = rect.width() * progress.clamp(0.0, 1.0);
    let progress_rect =
        egui::Rect::from_min_size(rect.min, Vec2::new(progress_width, rect.height()));
    ui.painter()
        .rect_filled(progress_rect, radius::SMALL, ui.visuals().selection.bg_fill);

    // Label
    if let Some(text) = label {
        ui.label(
            egui::RichText::new(text)
                .small()
                .color(ui.visuals().text_color()),
        );
    }
}

/// Empty state placeholder
pub fn empty_state(ui: &mut egui::Ui, icon: &str, title: &str, message: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(spacing::SECTION_SPACING);
        ui.label(
            egui::RichText::new(icon)
                .size(48.0)
                .color(ui.visuals().weak_text_color()),
        );
        ui.add_space(spacing::ITEM_SPACING);
        ui.label(egui::RichText::new(title).strong());
        ui.label(egui::RichText::new(message).color(ui.visuals().text_color()));
        ui.add_space(spacing::SECTION_SPACING);
    });
}

/// Custom style for egui_dock to match CodeStory theme
pub fn dock_style(ctx: &egui::Context) -> DockStyle {
    let mut style = DockStyle::from_egui(&ctx.style());

    // Customize for professional look using current visuals
    let visuals = &ctx.style().visuals;

    style.tab.active.bg_fill = visuals.selection.bg_fill;
    style.tab.active.text_color = visuals.strong_text_color();

    style.tab.inactive.bg_fill = visuals.faint_bg_color;
    style.tab.inactive.text_color = visuals.text_color();

    style.tab.hovered.bg_fill = visuals.widgets.hovered.bg_fill;
    style.tab.hovered.text_color = visuals.strong_text_color();

    style.buttons.close_tab_color = visuals.text_color();
    style.buttons.close_tab_active_color = visuals.error_fg_color;

    style.tab_bar.hline_color = visuals.selection.bg_fill;
    style.tab_bar.bg_fill = visuals.window_fill;

    style
}
