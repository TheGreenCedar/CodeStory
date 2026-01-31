//! CodeStory Theme and UI Polish
//!
//! Provides consistent styling, colors, and visual polish across the application.
//! Now powered by catppuccin-egui.

use eframe::egui::{self, Color32, Vec2};
use egui_dock::Style as DockStyle;
use egui_phosphor::regular as ph;
use std::path::{Path, PathBuf};

use crate::settings::{PhosphorVariant, ThemeMode, UiFontFamily};

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
    pub mode: ThemeMode,
    pub flavor: catppuccin_egui::Theme,

    /// Font customization fields
    pub font_size_base: f32,
    pub font_size_small: f32,
    pub font_size_heading: f32,
    pub show_icons: bool,
    pub compact_mode: bool,
    pub animation_speed: f32,
    pub ui_font_family: UiFontFamily,
    pub phosphor_variant: PhosphorVariant,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            mode: ThemeMode::Bright,
            flavor: catppuccin_egui::LATTE,
            font_size_base: 13.0,
            font_size_small: 11.0,
            font_size_heading: 16.0,
            show_icons: true,
            compact_mode: false,
            animation_speed: 1.0,
            ui_font_family: UiFontFamily::SourceCodePro,
            phosphor_variant: PhosphorVariant::Regular,
        }
    }
}

impl Theme {
    pub fn new(mode: ThemeMode) -> Self {
        let flavor = match mode {
            ThemeMode::Latte => catppuccin_egui::LATTE,
            ThemeMode::Frappe => catppuccin_egui::FRAPPE,
            ThemeMode::Macchiato => catppuccin_egui::MACCHIATO,
            ThemeMode::Mocha => catppuccin_egui::MOCHA,
            ThemeMode::Bright => catppuccin_egui::LATTE,
            ThemeMode::Dark => catppuccin_egui::MOCHA,
        };

        Self {
            mode,
            flavor,
            ..Default::default()
        }
    }

    /// Apply theme to egui context
    pub fn apply(&self, ctx: &egui::Context) {
        match self.mode {
            ThemeMode::Bright | ThemeMode::Dark => apply_sourcetrail_visuals(ctx, self.mode),
            _ => catppuccin_egui::set_theme(ctx, self.flavor),
        }
        self.setup_fonts(ctx);
    }

    fn setup_fonts(&self, ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();

        // Load Sourcetrail fonts if available
        if let Some(font_dir) = find_sourcetrail_fonts_dir() {
            load_font(&mut fonts, "SourceCodePro-Regular", &font_dir, "SourceCodePro-Regular.otf");
            load_font(&mut fonts, "SourceCodePro-Medium", &font_dir, "SourceCodePro-Medium.otf");
            load_font(&mut fonts, "SourceCodePro-Bold", &font_dir, "SourceCodePro-Bold.otf");
            load_font(&mut fonts, "FiraSans-Regular", &font_dir, "FiraSans-Regular.otf");
            load_font(&mut fonts, "FiraSans-SemiBold", &font_dir, "FiraSans-SemiBold.otf");
            load_font(&mut fonts, "Roboto-Regular", &font_dir, "Roboto-Regular.ttf");
            load_font(&mut fonts, "Roboto-Bold", &font_dir, "Roboto-Bold.ttf");
        }

        let phosphor_variant = match self.phosphor_variant {
            PhosphorVariant::Regular => egui_phosphor::Variant::Regular,
            PhosphorVariant::Bold => egui_phosphor::Variant::Bold,
            PhosphorVariant::Fill => egui_phosphor::Variant::Fill,
            PhosphorVariant::Light => egui_phosphor::Variant::Light,
            PhosphorVariant::Thin => egui_phosphor::Variant::Thin,
        };
        egui_phosphor::add_to_fonts(&mut fonts, phosphor_variant);

        // Set primary UI font
        let ui_font = match self.ui_font_family {
            UiFontFamily::SourceCodePro => "SourceCodePro-Regular",
            UiFontFamily::FiraSans => "FiraSans-Regular",
            UiFontFamily::Roboto => "Roboto-Regular",
        };

        if fonts.font_data.contains_key(ui_font) {
            if let Some(family) = fonts
                .families
                .get_mut(&egui::FontFamily::Proportional)
            {
                family.insert(0, ui_font.to_string());
            }
        }

        // Force Source Code Pro for monospace if available
        if fonts.font_data.contains_key("SourceCodePro-Regular") {
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                family.insert(0, "SourceCodePro-Regular".to_string());
            }
        }

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

fn apply_sourcetrail_visuals(ctx: &egui::Context, mode: ThemeMode) {
    let mut visuals = match mode {
        ThemeMode::Bright => egui::Visuals::light(),
        ThemeMode::Dark => egui::Visuals::dark(),
        _ => egui::Visuals::dark(),
    };

    let (window_bg, separator, dock_bg, dock_text, button_bg, button_border, button_hover) =
        match mode {
            ThemeMode::Bright => (
                hex_color("#FFFFFF"),
                hex_color("#A2A2A2"),
                hex_color("#E0E0E0"),
                hex_color("#000000"),
                hex_color("#FFFFFF"),
                hex_color("#AAAAAA"),
                hex_color("#F0F0F0"),
            ),
            ThemeMode::Dark => (
                hex_color("#272728"),
                hex_color("#CCCCCC"),
                hex_color("#555555"),
                hex_color("#F7F7F7"),
                hex_color("#3A3A3A"),
                hex_color("#C5C5C5"),
                hex_color("#404040"),
            ),
            _ => (
                hex_color("#272728"),
                hex_color("#CCCCCC"),
                hex_color("#555555"),
                hex_color("#F7F7F7"),
                hex_color("#3A3A3A"),
                hex_color("#C5C5C5"),
                hex_color("#404040"),
            ),
        };

    visuals.window_fill = window_bg;
    visuals.panel_fill = window_bg;
    visuals.widgets.noninteractive.bg_fill = dock_bg;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, separator);
    visuals.widgets.noninteractive.fg_stroke.color = dock_text;

    visuals.widgets.inactive.bg_fill = button_bg;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, button_border);
    visuals.widgets.inactive.fg_stroke.color = dock_text;

    visuals.widgets.hovered.bg_fill = button_hover;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, separator);
    visuals.widgets.hovered.fg_stroke.color = dock_text;

    visuals.widgets.active.bg_fill = button_bg;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, separator);
    visuals.widgets.active.fg_stroke.color = dock_text;

    visuals.override_text_color = Some(dock_text);
    visuals.window_stroke = egui::Stroke::new(1.0, separator);
    visuals.selection.bg_fill = separator;

    ctx.set_visuals(visuals);
}

fn hex_color(hex: &str) -> Color32 {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        Color32::from_rgb(r, g, b)
    } else if hex.len() == 8 {
        let a = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255);
        let r = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[6..8], 16).unwrap_or(0);
        Color32::from_rgba_unmultiplied(r, g, b, a)
    } else {
        Color32::WHITE
    }
}

fn find_sourcetrail_fonts_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CODESTORY_FONT_DIR") {
        let path = PathBuf::from(dir);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(dir) = std::env::var("CODESTORY_SOURCETRAIL_DIR") {
        let path = PathBuf::from(dir).join("bin").join("app").join("data").join("fonts");
        if path.exists() {
            return Some(path);
        }
    }

    let mut current = std::env::current_dir().ok()?;
    for _ in 0..6 {
        let candidate = current
            .join("Sourcetrail")
            .join("bin")
            .join("app")
            .join("data")
            .join("fonts");
        if candidate.exists() {
            return Some(candidate);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn load_font(fonts: &mut egui::FontDefinitions, name: &str, dir: &Path, file: &str) {
    let path = dir.join(file);
    if let Ok(bytes) = std::fs::read(path) {
        fonts
            .font_data
            .insert(name.to_string(), egui::FontData::from_owned(bytes).into());
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
            ui.label(egui::RichText::new(ph::INFO).color(fg));
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
            ui.label(egui::RichText::new(ph::WARNING).color(fg));
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
            ui.label(egui::RichText::new(ph::X_CIRCLE).color(fg));
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
