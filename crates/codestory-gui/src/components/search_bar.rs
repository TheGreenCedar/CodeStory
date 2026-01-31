use crate::theme::{self, badge};
use codestory_core::NodeId;
use eframe::egui;
use egui_phosphor::regular as ph;

/// A search result item for autocomplete
#[derive(Debug, Clone)]
pub struct SearchMatch {
    /// Node ID for navigation
    pub node_id: NodeId,
    /// Display name (e.g., "MyClass", "my_function")
    pub name: String,
    /// Full qualified name (e.g., "mymodule::MyClass")
    pub qualified_name: String,
    /// Type of the match (e.g., "class", "function", "file")
    pub kind: String,
    /// File path where this symbol is defined
    pub file_path: Option<String>,
    /// Line number
    pub line: Option<u32>,
    /// Score for ranking (higher is better)
    pub score: f32,
}

/// Search bar with autocomplete dropdown
pub struct SearchBar {
    query: String,
    /// Autocomplete results
    pub suggestions: Vec<SearchMatch>,
    /// Currently selected suggestion index
    pub selected_index: usize,
    /// Whether the dropdown is visible
    pub show_dropdown: bool,
    /// Whether we're waiting for results
    pub is_loading: bool,
    /// Last query that was sent for autocomplete
    last_autocomplete_query: String,
}

impl SearchBar {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            suggestions: Vec::new(),
            selected_index: 0,
            show_dropdown: false,
            is_loading: false,
            last_autocomplete_query: String::new(),
        }
    }

    /// Set the autocomplete suggestions
    pub fn set_suggestions(&mut self, suggestions: Vec<SearchMatch>) {
        self.suggestions = suggestions;
        self.selected_index = 0;
        self.is_loading = false;
        self.show_dropdown = !self.suggestions.is_empty();
    }

    /// Clear suggestions and hide dropdown
    pub fn clear_suggestions(&mut self) {
        self.suggestions.clear();
        self.selected_index = 0;
        self.show_dropdown = false;
    }

    /// Get the current query
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Set the query programmatically
    pub fn set_query(&mut self, query: String) {
        self.query = query;
    }

    /// Render the search bar with autocomplete
    /// Returns SearchAction if an action was triggered
    pub fn ui(&mut self, ui: &mut egui::Ui) -> SearchAction {
        let mut action = SearchAction::None;
        let mut input_rect = egui::Rect::NOTHING;

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(ph::MAGNIFYING_GLASS).color(ui.visuals().selection.bg_fill));

            // Text input
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Search symbols...")
                    .desired_width(300.0),
            );
            input_rect = response.rect;

            // Handle keyboard navigation
            if response.has_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) && !self.suggestions.is_empty()
                {
                    self.selected_index = (self.selected_index + 1) % self.suggestions.len();
                }
                if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) && !self.suggestions.is_empty() {
                    if self.selected_index == 0 {
                        self.selected_index = self.suggestions.len() - 1;
                    } else {
                        self.selected_index -= 1;
                    }
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if self.show_dropdown && !self.suggestions.is_empty() {
                        // Select current suggestion
                        action = SearchAction::SelectMatch(
                            self.suggestions[self.selected_index].clone(),
                        );
                        self.show_dropdown = false;
                    } else {
                        // Full search
                        action = SearchAction::FullSearch(self.query.clone());
                    }
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.show_dropdown = false;
                }
            }

            // Trigger autocomplete when query changes
            if response.changed() && self.query.len() >= 2 {
                if self.query != self.last_autocomplete_query {
                    self.last_autocomplete_query = self.query.clone();
                    self.is_loading = true;
                    action = SearchAction::Autocomplete(self.query.clone());
                }
            } else if self.query.len() < 2 {
                self.clear_suggestions();
            }

            // Loading indicator
            if self.is_loading {
                ui.spinner();
            }

            // Search button
            if ui.add(theme::primary_button(ui, "Search")).clicked() {
                action = SearchAction::FullSearch(self.query.clone());
                self.show_dropdown = false;
            }
        });

        // Show autocomplete dropdown
        if self.show_dropdown && !self.suggestions.is_empty() && input_rect != egui::Rect::NOTHING {
            let ctx = ui.ctx().clone();
            let dropdown_response = self.render_dropdown(&ctx, input_rect);
            if let Some(selected) = dropdown_response {
                action = SearchAction::SelectMatch(selected);
                self.show_dropdown = false;
            }
        }

        action
    }

    fn render_dropdown(
        &mut self,
        ctx: &egui::Context,
        input_rect: egui::Rect,
    ) -> Option<SearchMatch> {
        let mut selected_match = None;

        // Position the dropdown below the search bar using Area
        egui::Area::new("search_autocomplete_area".into())
            .fixed_pos(input_rect.left_bottom())
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(input_rect.width().max(400.0));
                    ui.set_max_height(300.0);

                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (idx, suggestion) in self.suggestions.iter().enumerate() {
                            let is_selected = idx == self.selected_index;

                            // Use a selectable frame for proper hover/selection visuals
                            let row_response = ui
                                .push_id(idx, |ui| {
                                    let available_width = ui.available_width();
                                    let (rect, response) = ui.allocate_at_least(
                                        egui::vec2(available_width, 24.0),
                                        egui::Sense::click(),
                                    );

                                    // Determine background color based on state
                                    let bg_color = if response.hovered() || is_selected {
                                        ui.visuals().widgets.hovered.bg_fill
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    };

                                    // Draw background first
                                    ui.painter().rect_filled(rect, 2.0, bg_color);

                                    // Now draw content on top using a child UI
                                    let mut content_rect = rect;
                                    content_rect.min.x += 4.0;
                                    content_rect.max.x -= 4.0;

                                    let mut child_ui =
                                        ui.new_child(egui::UiBuilder::new().max_rect(content_rect));
                                    child_ui.horizontal_centered(|ui| {
                                        // Kind badge with theme colors
                                        let kind_color = match suggestion.kind.as_str() {
                                            "class" | "struct" => ui.visuals().selection.bg_fill,
                                            "function" | "method" | "fn" => {
                                                ui.visuals().warn_fg_color
                                            }
                                            "variable" | "field" | "var" => {
                                                ui.visuals().selection.bg_fill
                                            }
                                            "file" => egui::Color32::LIGHT_GREEN,
                                            "module" | "mod" | "package" => {
                                                ui.visuals().text_color()
                                            }
                                            _ => ui.visuals().weak_text_color(),
                                        };

                                        badge(ui, &suggestion.kind, kind_color);

                                        // Name (highlighted) - truncate if too long
                                        let display_name = if suggestion.name.chars().count() > 30 {
                                            let truncated: String =
                                                suggestion.name.chars().take(27).collect();
                                            format!("{}...", truncated)
                                        } else {
                                            suggestion.name.clone()
                                        };
                                        ui.label(egui::RichText::new(display_name).strong())
                                            .on_hover_text(&suggestion.name);

                                        // Qualified name if different - truncate if too long
                                        if suggestion.qualified_name != suggestion.name {
                                            let display_qualified =
                                                if suggestion.qualified_name.chars().count() > 40 {
                                                    let truncated: String = suggestion
                                                        .qualified_name
                                                        .chars()
                                                        .take(37)
                                                        .collect();
                                                    format!("{}...", truncated)
                                                } else {
                                                    suggestion.qualified_name.clone()
                                                };
                                            ui.label(
                                                egui::RichText::new(display_qualified)
                                                    .small()
                                                    .weak(),
                                            )
                                            .on_hover_text(&suggestion.qualified_name);
                                        }

                                        // Line number if available
                                        if let Some(line) = suggestion.line {
                                            ui.label(
                                                egui::RichText::new(format!("L{}", line))
                                                    .small()
                                                    .weak(),
                                            );
                                        }

                                        // File path and score on right
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                // Score indicator (star rating based on score)
                                                let stars = ((suggestion.score * 5.0).round()
                                                    as usize)
                                                    .min(5);
                                                if stars > 0 {
                                                    ui.label(
                                                        egui::RichText::new(ph::STAR.repeat(stars))
                                                            .small()
                                                            .color(ui.visuals().warn_fg_color),
                                                    );
                                                }

                                                // File path
                                                if let Some(path) = &suggestion.file_path {
                                                    let display_path = if path.chars().count() > 45
                                                    {
                                                        let truncated: String = path
                                                            .chars()
                                                            .skip(path.chars().count() - 42)
                                                            .collect();
                                                        format!("...{}", truncated)
                                                    } else {
                                                        path.clone()
                                                    };

                                                    ui.label(
                                                        egui::RichText::new(display_path)
                                                            .small()
                                                            .weak(),
                                                    );
                                                }
                                            },
                                        );
                                    });

                                    response
                                })
                                .inner;

                            if row_response.clicked() {
                                selected_match = Some(suggestion.clone());
                            }

                            if row_response.hovered() {
                                self.selected_index = idx;
                            }
                        }
                    });
                });
            });

        selected_match
    }
}

/// Actions that can result from search bar interaction
#[derive(Debug, Clone)]
pub enum SearchAction {
    /// No action
    None,
    /// Request autocomplete for query
    Autocomplete(String),
    /// Perform full search for query
    FullSearch(String),
    /// User selected a match from autocomplete
    SelectMatch(SearchMatch),
}
