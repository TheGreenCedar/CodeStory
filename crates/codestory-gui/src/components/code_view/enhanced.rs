use codestory_core::SourceLocation;
use eframe::egui;
use egui_code_editor::{CodeEditor, ColorTheme, Syntax};
use std::ops::Range;

/// Language syntax types supported
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Cpp,
    C,
    Python,
    Java,
    JavaScript,
    TypeScript,
    Unknown,
}

impl Language {
    /// Detect language from file extension
    pub fn from_path(path: &str) -> Self {
        let path = std::path::Path::new(path);
        match path.extension().and_then(|e| e.to_str()) {
            Some("rs") => Language::Rust,
            Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => Language::Cpp,
            Some("c") | Some("h") => Language::C,
            Some("py") => Language::Python,
            Some("java") => Language::Java,
            Some("js") | Some("jsx") => Language::JavaScript,
            Some("ts") | Some("tsx") => Language::TypeScript,
            _ => Language::Unknown,
        }
    }

    /// Convert to egui_code_editor syntax
    pub fn to_syntax(self) -> Syntax {
        match self {
            Language::Rust => Syntax::rust(),
            Language::Cpp => Syntax::rust(),
            Language::C => Syntax::shell(),
            Language::Python => Syntax::python(),
            Language::Java => Syntax::rust(),
            Language::JavaScript => Syntax::rust(),
            Language::TypeScript => Syntax::rust(),
            Language::Unknown => Syntax::shell(),
        }
    }
}

pub struct EnhancedCodeView {
    pub path: String,
    pub content: String,
    pub language: Language,
    pub target_line: Option<usize>,
    pub scroll_to_line: Option<(usize, u8)>, // (line, retries_left)

    // Editor state
    pub font_size: f32,

    // For selection/navigation
    pub selection: Option<Range<usize>>,
    pub active_locations: Vec<SourceLocation>,
    pub occurrences: Vec<codestory_core::Occurrence>,

    // Search state
    pub search_query: String,
    pub search_active: bool,

    /// Last reported center line for scroll sync
    pub last_visible_center_line: Option<usize>,

    pub event_bus: Option<codestory_events::EventBus>,
}

impl EnhancedCodeView {
    pub fn new() -> Self {
        Self {
            path: String::new(),
            content: String::new(),
            language: Language::Unknown,
            target_line: None,
            scroll_to_line: None,
            font_size: 14.0,
            selection: None,
            active_locations: Vec::new(),
            occurrences: Vec::new(),
            search_query: String::new(),
            search_active: false,
            last_visible_center_line: None,
            event_bus: None,
        }
    }

    pub fn set_file(&mut self, path: String, content: String, line: usize) {
        self.language = Language::from_path(&path);
        self.path = path;
        self.content = content;
        self.target_line = Some(line);
        self.scroll_to_line = Some((line, 5)); // Retry for 5 frames
        self.selection = None;
        self.active_locations.clear();
        self.occurrences.clear();
        // Keep search active if it was open, but maybe clear matches logic (not needed as content changes)
        // self.search_query.clear(); // Optional: keep query persistence? Let's keep it.
    }

    pub fn show_location(&mut self, path: String, content: String, location: SourceLocation) {
        self.set_file(path, content, location.start_line as usize);
        self.active_locations.push(location);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, _theme: &crate::theme::Theme) {
        // Use a stable, unique ID for the code editor's scroll area
        let scroll_area_id = format!("enhanced_code_scroll_{}", self.path);

        let mut editor = CodeEditor::default()
            .with_fontsize(self.font_size)
            .with_theme(ColorTheme::GRUVBOX)
            .with_syntax(self.language.to_syntax())
            .with_numlines(true)
            .vscroll(false); // We handle scrolling ourselves

        // Compute row height once
        let font_id = egui::FontId::monospace(self.font_size);
        let row_height = ui
            .painter()
            .layout_no_wrap("A".to_string(), font_id.clone(), egui::Color32::TRANSPARENT)
            .rect
            .height();

        const TOP_PADDING: f32 = 4.0;

        // Wrap the editor in a ScrollArea that we control
        egui::ScrollArea::vertical()
            .id_salt(&scroll_area_id)
            .show(ui, |ui| {
                // Show the editor
                let response = editor.show(ui, &mut self.content);

                // --- 1. Handle Targeted Scrolling ---
                if let Some((target_line, retries)) = self.scroll_to_line {
                    if retries > 0 {
                        // Calculate target Y position relative to editor top
                        let y_offset =
                            (target_line.saturating_sub(1) as f32) * row_height + TOP_PADDING;

                        // Target rect (one line height)
                        let scroll_rect = egui::Rect::from_min_size(
                            response.response.rect.min + egui::vec2(0.0, y_offset),
                            egui::vec2(response.response.rect.width(), row_height),
                        );

                        // Trigger the scroll
                        ui.scroll_to_rect(scroll_rect, Some(egui::Align::Center));

                        // Continue retrying to ensure layout stability
                        self.scroll_to_line = Some((target_line, retries - 1));
                        ui.ctx().request_repaint();
                    } else {
                        self.scroll_to_line = None;
                    }
                } else if let Some(target) = self.target_line.take() {
                    // Initialize scroll retry loop
                    self.scroll_to_line = Some((target, 10)); // Increased retries for stability
                }

                // --- 2. Handle Highlights ---
                let painter = ui.painter();
                let active_color = ui.visuals().selection.bg_fill;
                let occurrence_color = ui.visuals().selection.bg_fill.linear_multiply(0.3);

                // Search Matches Highlight
                if self.search_active && !self.search_query.is_empty() {
                    let query = &self.search_query.to_lowercase();
                    let match_color = egui::Color32::from_rgba_premultiplied(255, 255, 0, 40); // Yellow tint

                    for (i, line) in self.content.lines().enumerate() {
                        let line_lower = line.to_lowercase();
                        let mut start_idx = 0;
                        while let Some(idx) = line_lower[start_idx..].find(query) {
                            let match_start = start_idx + idx;
                            let match_end = match_start + query.len();
                            // Simple line highlight for now (enhancing later for precise character highlighting needs TextLayout)
                            // Just highlight the whole line where there is a match to keep it simple and performant

                            let y_start = (i as f32) * row_height + TOP_PADDING;
                            let rect = egui::Rect::from_min_size(
                                response.response.rect.min + egui::vec2(0.0, y_start),
                                egui::vec2(response.response.rect.width(), row_height),
                            );

                            let clipped_rect = rect.intersect(response.response.rect);
                            if clipped_rect.is_positive() {
                                painter.rect_filled(clipped_rect, 0.0, match_color);
                            }

                            start_idx = match_end;
                        }
                    }
                }

                // Occurrences
                for occurrence in &self.occurrences {
                    let start_line = occurrence.location.start_line.saturating_sub(1) as f32;
                    let end_line = occurrence.location.end_line.saturating_sub(1) as f32;
                    let y_start = start_line * row_height + TOP_PADDING;
                    let height = (end_line - start_line + 1.0) * row_height;

                    let rect = egui::Rect::from_min_size(
                        response.response.rect.min + egui::vec2(0.0, y_start),
                        egui::vec2(response.response.rect.width(), height),
                    );

                    let clipped_rect = rect.intersect(response.response.rect);
                    if clipped_rect.is_positive() {
                        painter.rect_filled(clipped_rect, 0.0, occurrence_color);
                    }
                }

                // Active Symbol Location(s)
                for location in &self.active_locations {
                    let start_line = location.start_line.saturating_sub(1) as f32;
                    let end_line = location.end_line.saturating_sub(1) as f32;
                    let y_start = start_line * row_height + TOP_PADDING;
                    let height = (end_line - start_line + 1.0) * row_height;

                    let rect = egui::Rect::from_min_size(
                        response.response.rect.min + egui::vec2(0.0, y_start),
                        egui::vec2(response.response.rect.width(), height),
                    );

                    let clipped_rect = rect.intersect(response.response.rect);
                    if clipped_rect.is_positive() {
                        painter.rect_filled(clipped_rect, 0.0, active_color);
                    }
                }

                // --- 3. Scroll Sync Reporting ---
                let viewport = ui.clip_rect();
                let editor_rect = response.response.rect;

                // Calculate which line is at the vertical center of the viewport
                let viewport_center_y = viewport.center().y;
                let scroll_offset_y = viewport_center_y - editor_rect.min.y;
                let center_line =
                    ((scroll_offset_y - TOP_PADDING) / row_height).round() as usize + 1;

                if self.last_visible_center_line != Some(center_line) {
                    self.last_visible_center_line = Some(center_line);
                    if let Some(event_bus) = &self.event_bus {
                        event_bus.publish(codestory_events::Event::CodeVisibleLineChanged {
                            file: self.path.clone(),
                            line: center_line,
                        });
                    }
                }
            });

        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.search_active = !self.search_active;
            if self.search_active {
                ui.ctx().request_repaint(); // Focus grab requires repaint usually
            }
        }

        if self.search_active {
            // Anchor to the top-right of the visible area
            let base_pos = ui.clip_rect().min;
            let available_width = ui.clip_rect().width();

            // Draw overlay relative to the overall UI
            ui.put(
                egui::Rect::from_min_size(
                    base_pos + egui::vec2(available_width - 250.0, 5.0),
                    egui::vec2(240.0, 40.0),
                ),
                |ui: &mut egui::Ui| {
                    egui::Frame::popup(ui.style())
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("🔍");
                                let res = ui.text_edit_singleline(&mut self.search_query);
                                if res.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    // Find next (TODO)
                                }
                                if self.search_query.is_empty() {
                                    res.request_focus();
                                }

                                if ui.button("X").clicked() {
                                    self.search_active = false;
                                    self.search_query.clear();
                                }
                            });
                        })
                        .response
                },
            );
        }
    }
}
