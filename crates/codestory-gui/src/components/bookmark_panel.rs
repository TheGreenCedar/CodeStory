//! Bookmark Panel Component
//!
//! Manages bookmark categories and individual bookmarks with navigation support.

use crate::theme::{self, card, empty_state, spacing};
use codestory_core::{Bookmark, BookmarkCategory, NodeId};
use codestory_events::{ActivationOrigin, Event, EventBus};
use eframe::egui;
use egui_phosphor::regular as ph;

pub struct BookmarkPanel {
    pub categories: Vec<BookmarkCategory>,
    pub bookmarks: Vec<Bookmark>,
    pub expanded_categories: std::collections::HashSet<i64>,
    pub editing_comment: Option<(i64, String)>, // (bookmark_id, current_text)
    pub new_category_name: String,
    pub show_add_category: bool,
    event_bus: EventBus,
}

impl BookmarkPanel {
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            categories: Vec::new(),
            bookmarks: Vec::new(),
            expanded_categories: std::collections::HashSet::new(),
            editing_comment: None,
            new_category_name: String::new(),
            show_add_category: false,
            event_bus,
        }
    }

    pub fn set_data(&mut self, categories: Vec<BookmarkCategory>, bookmarks: Vec<Bookmark>) {
        self.categories = categories;
        self.bookmarks = bookmarks;
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        node_names: &std::collections::HashMap<NodeId, String>,
    ) {
        // Header
        ui.horizontal(|ui| {
            ui.heading(egui::RichText::new("Bookmarks").color(ui.visuals().selection.bg_fill));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(theme::icon_button("+"))
                    .on_hover_text("Add category")
                    .clicked()
                {
                    self.show_add_category = true;
                }
            });
        });

        ui.separator();

        // Add category dialog
        if self.show_add_category {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    let response = ui.text_edit_singleline(&mut self.new_category_name);
                    if response.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && !self.new_category_name.is_empty()
                    {
                        self.event_bus.publish(Event::BookmarkCategoryCreate {
                            name: self.new_category_name.clone(),
                        });
                        self.new_category_name.clear();
                        self.show_add_category = false;
                    }
                    if ui.add(theme::secondary_button(ui, "Cancel")).clicked() {
                        self.show_add_category = false;
                        self.new_category_name.clear();
                    }
                });
            });
            ui.add_space(spacing::ITEM_SPACING);
        }

        // Categories and bookmarks
        if self.categories.is_empty() {
            empty_state(ui, ph::BOOKMARK, "No Bookmark Categories", "Click + to create one");
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            for category in &self.categories.clone() {
                self.render_category(ui, category, node_names);
            }
        });
    }

    fn render_category(
        &mut self,
        ui: &mut egui::Ui,
        category: &BookmarkCategory,
        node_names: &std::collections::HashMap<NodeId, String>,
    ) {
        let is_expanded = self.expanded_categories.contains(&category.id);
        let category_bookmarks: Vec<Bookmark> = self
            .bookmarks
            .iter()
            .filter(|b| b.category_id == category.id)
            .cloned()
            .collect();
        let bookmark_count = category_bookmarks.len();

        // Category header
        let header_response = ui.horizontal(|ui| {
            let icon = if is_expanded {
                ph::CARET_DOWN
            } else {
                ph::CARET_RIGHT
            };
            if ui.small_button(icon).clicked() {
                if is_expanded {
                    self.expanded_categories.remove(&category.id);
                } else {
                    self.expanded_categories.insert(category.id);
                }
            }

            ui.label(
                egui::RichText::new(format!("{} ({})", category.name, bookmark_count)).strong(),
            );
        });

        // Context menu for category
        header_response.response.context_menu(|ui| {
            if ui.button("Delete category").clicked() {
                self.event_bus
                    .publish(Event::BookmarkCategoryDelete { id: category.id });
                ui.close();
            }
        });

        // Bookmarks in category
        if is_expanded {
            ui.indent(category.id, |ui| {
                if category_bookmarks.is_empty() {
                    ui.label(egui::RichText::new("No bookmarks").weak().small());
                } else {
                    for bookmark in category_bookmarks {
                        self.render_bookmark(ui, &bookmark, node_names);
                    }
                }
            });
        }
    }

    fn render_bookmark(
        &mut self,
        ui: &mut egui::Ui,
        bookmark: &Bookmark,
        node_names: &std::collections::HashMap<NodeId, String>,
    ) {
        let node_name = node_names
            .get(&bookmark.node_id)
            .cloned()
            .unwrap_or_else(|| format!("Node {}", bookmark.node_id.0));

        let is_editing = self
            .editing_comment
            .as_ref()
            .map(|(id, _)| *id == bookmark.id)
            .unwrap_or(false);

        ui.horizontal(|ui| {
            // Bookmark icon
            ui.label("*");

            // Node name (clickable)
            if ui.link(&node_name).clicked() {
                self.event_bus.publish(Event::ActivateNode {
                    id: bookmark.node_id,
                    origin: ActivationOrigin::Sidebar,
                });
            }

            // Comment
            if is_editing {
                if let Some((_, ref mut text)) = self.editing_comment {
                    let response = ui.text_edit_singleline(text);
                    if response.lost_focus() {
                        // Save comment
                        // Note: In a real implementation, we'd emit an event here
                        self.editing_comment = None;
                    }
                }
            } else if let Some(comment) = &bookmark.comment {
                ui.label(egui::RichText::new(comment).weak().small());
            }

            // Actions
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("x").on_hover_text("Delete").clicked() {
                    self.event_bus
                        .publish(Event::BookmarkRemove { id: bookmark.id });
                }
                if ui.small_button("e").on_hover_text("Edit comment").clicked() {
                    self.editing_comment =
                        Some((bookmark.id, bookmark.comment.clone().unwrap_or_default()));
                }
            });
        });
    }

    /// Add a bookmark to the first available category (or create "Default" if none exist)
    /// Available for use from context menus in other components.
    pub fn add_bookmark_to_default(&self, node_id: NodeId) {
        if let Some(category) = self.categories.first() {
            self.event_bus.publish(Event::BookmarkAdd {
                node_id,
                category_id: category.id,
            });
        } else {
            // Create default category first
            self.event_bus.publish(Event::BookmarkCategoryCreate {
                name: "Default".to_string(),
            });
            // Note: The actual bookmark add would need to happen after category creation
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bookmark_panel_creation() {
        let event_bus = codestory_events::EventBus::new();
        let panel = BookmarkPanel::new(event_bus);
        assert!(panel.categories.is_empty());
        assert!(panel.bookmarks.is_empty());
    }

    #[test]
    fn test_set_data() {
        let event_bus = codestory_events::EventBus::new();
        let mut panel = BookmarkPanel::new(event_bus);

        let categories = vec![BookmarkCategory {
            id: 1,
            name: "Test".to_string(),
        }];
        let bookmarks = vec![Bookmark {
            id: 1,
            category_id: 1,
            node_id: NodeId(100),
            comment: Some("Test comment".to_string()),
        }];

        panel.set_data(categories, bookmarks);

        assert_eq!(panel.categories.len(), 1);
        assert_eq!(panel.bookmarks.len(), 1);
    }
}
