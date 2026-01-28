use crate::theme::{empty_state, spacing};
use codestory_core::{Node, NodeId};
use codestory_storage::Storage;
use eframe::egui;
use std::path::{Path, PathBuf};

/// Maximum number of symbol nodes to cache children for.
/// This prevents unbounded memory growth in very large projects.
const MAX_SYMBOL_CACHE_SIZE: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Files,
    Symbols,
}

pub enum SidebarAction {
    FileSelected,
    NodeSelected,
}

/// Sidebar component for browsing files and symbols.
///
/// ## Performance Optimization
///
/// The symbol tree uses lazy loading to improve performance with large codebases:
/// - Root symbols are loaded immediately when switching to the Symbols tab
/// - Children are loaded only when a parent node is expanded
/// - Loaded children are cached to avoid repeated database queries
/// - Cache size is limited to prevent unbounded memory growth
///
/// This approach reduces initial load time and memory usage, especially for
/// projects with thousands of symbols.
pub struct Sidebar {
    pub root_path: Option<PathBuf>,
    pub selected_path: Option<PathBuf>,
    pub active_tab: SidebarTab,
    pub selected_node: Option<NodeId>,
    /// Cache of loaded symbol children to avoid re-querying storage
    /// Key: parent NodeId, Value: Vec of child nodes
    symbol_children_cache: std::collections::HashMap<NodeId, Vec<Node>>,
    /// Set of nodes whose children have been loaded
    children_loaded: std::collections::HashSet<NodeId>,
}

impl Sidebar {
    pub fn new() -> Self {
        Self {
            root_path: None,
            selected_path: None,
            active_tab: SidebarTab::Files,
            selected_node: None,
            symbol_children_cache: std::collections::HashMap::new(),
            children_loaded: std::collections::HashSet::new(),
        }
    }

    /// Clear symbol caches (call when reindexing or switching projects)
    pub fn clear_symbol_cache(&mut self) {
        self.symbol_children_cache.clear();
        self.children_loaded.clear();
    }

    /// Load children for a symbol node if not already cached.
    ///
    /// This implements lazy loading: children are only fetched from storage when
    /// a node is expanded for the first time. Subsequent expansions use the cached data.
    ///
    /// Performance benefit: For a project with 10,000 symbols, this avoids loading
    /// all children upfront, instead loading only what's visible (typically < 100 nodes).
    fn ensure_children_loaded(&mut self, storage: &Storage, parent_id: NodeId) {
        // Check if already loaded
        if !self.children_loaded.contains(&parent_id) {
            // Prevent unbounded cache growth in very large projects
            if self.symbol_children_cache.len() >= MAX_SYMBOL_CACHE_SIZE {
                tracing::debug!(
                    "Symbol cache size limit reached ({}), clearing oldest entries",
                    MAX_SYMBOL_CACHE_SIZE
                );
                // Simple eviction: clear half the cache
                // More sophisticated LRU could be implemented if needed
                let to_remove: Vec<NodeId> = self
                    .symbol_children_cache
                    .keys()
                    .take(MAX_SYMBOL_CACHE_SIZE / 2)
                    .copied()
                    .collect();
                for key in to_remove {
                    self.symbol_children_cache.remove(&key);
                    self.children_loaded.remove(&key);
                }
            }

            // Load from storage
            if let Ok(children) = storage.get_children_symbols(parent_id) {
                self.symbol_children_cache.insert(parent_id, children);
                self.children_loaded.insert(parent_id);
            } else {
                // Insert empty vec on error to avoid repeated queries
                self.symbol_children_cache.insert(parent_id, Vec::new());
                self.children_loaded.insert(parent_id);
            }
        }
    }

    pub fn set_root(&mut self, path: PathBuf) {
        self.root_path = Some(path);
    }

    pub fn set_selected_path(&mut self, path: PathBuf) {
        self.selected_path = Some(path);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, storage: Option<&Storage>) -> Option<SidebarAction> {
        let mut action = None;
        ui.vertical(|ui| {
            // Tab Header
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.active_tab == SidebarTab::Files, "Files")
                    .clicked()
                {
                    self.active_tab = SidebarTab::Files;
                }
                if ui
                    .selectable_label(self.active_tab == SidebarTab::Symbols, "Symbols")
                    .clicked()
                {
                    self.active_tab = SidebarTab::Symbols;
                }
            });
            ui.separator();

            match self.active_tab {
                SidebarTab::Files => {
                    if let Some(root) = self.root_path.clone() {
                        egui::ScrollArea::vertical()
                            .id_salt("sidebar_files_tree")
                            .show(ui, |ui| {
                                if let Some(p) = self.render_tree(ui, &root) {
                                    self.selected_path = Some(p.clone());
                                    action = Some(SidebarAction::FileSelected);
                                }
                            });
                    } else {
                        ui.add_space(spacing::SECTION_SPACING);
                        empty_state(ui, "📂", "No Project", "Open a folder to get started");
                    }
                }
                SidebarTab::Symbols => {
                    if let Some(storage) = storage {
                        egui::ScrollArea::vertical()
                            .id_salt("sidebar_symbols_tree")
                            .show(ui, |ui| {
                                if let Ok(roots) = storage.get_root_symbols() {
                                    for node in roots {
                                        if let Some(id) =
                                            self.render_symbol_node(ui, storage, &node)
                                        {
                                            self.selected_node = Some(id);
                                            action = Some(SidebarAction::NodeSelected);
                                        }
                                    }
                                }
                            });
                    } else {
                        ui.add_space(spacing::SECTION_SPACING);
                        empty_state(ui, "🧬", "No Symbols", "Open a project to index symbols");
                    }
                }
            }
        });
        action
    }

    fn render_symbol_node(
        &mut self,
        ui: &mut egui::Ui,
        storage: &Storage,
        node: &Node,
    ) -> Option<NodeId> {
        let mut selected = None;
        let icon = match node.kind {
            codestory_core::NodeKind::NAMESPACE | codestory_core::NodeKind::PACKAGE => "{} ",
            codestory_core::NodeKind::CLASS | codestory_core::NodeKind::STRUCT => "C ",
            codestory_core::NodeKind::INTERFACE => "I ",
            codestory_core::NodeKind::ENUM => "E ",
            codestory_core::NodeKind::METHOD | codestory_core::NodeKind::FUNCTION => "m ",
            codestory_core::NodeKind::FIELD => "f ",
            _ => "○ ",
        };

        let is_selected = self.selected_node == Some(node.id);
        let label_text = format!("{}{}", icon, node.serialized_name);

        // Check if node is a container type that can have children
        let is_container = matches!(
            node.kind,
            codestory_core::NodeKind::NAMESPACE
                | codestory_core::NodeKind::PACKAGE
                | codestory_core::NodeKind::CLASS
                | codestory_core::NodeKind::STRUCT
                | codestory_core::NodeKind::INTERFACE
                | codestory_core::NodeKind::ENUM
        );

        if is_container {
            let collapsing_state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                ui.make_persistent_id(node.id.0),
                false,
            );

            let is_open = collapsing_state.is_open();

            collapsing_state
                .show_header(ui, |ui| {
                    let resp = ui.selectable_label(is_selected, label_text);
                    if resp.clicked() {
                        selected = Some(node.id);
                    }
                })
                .body(|ui| {
                    // Lazy load children only when expanded
                    if is_open {
                        // Ensure children are loaded into cache
                        self.ensure_children_loaded(storage, node.id);

                        // Get children from cache and clone to avoid borrow issues
                        let children_clone: Vec<Node> = self
                            .symbol_children_cache
                            .get(&node.id)
                            .cloned()
                            .unwrap_or_default();

                        for child in children_clone {
                            if let Some(id) = self.render_symbol_node(ui, storage, &child) {
                                selected = Some(id);
                            }
                        }
                    }
                });
        } else {
            let resp = ui.selectable_label(is_selected, label_text);
            if resp.clicked() {
                selected = Some(node.id);
            }
        }

        selected
    }

    fn render_tree(&self, ui: &mut egui::Ui, path: &Path) -> Option<PathBuf> {
        let mut selected = None;
        let file_name = path.file_name()?.to_str()?;

        if path.is_dir() {
            let default_open = self
                .selected_path
                .as_ref()
                .map(|p| p.starts_with(path))
                .unwrap_or(false);
            egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                ui.make_persistent_id(path),
                default_open,
            )
            .show_header(ui, |ui| {
                ui.label(
                    egui::RichText::new(format!("📁 {}", file_name))
                        .color(ui.visuals().hyperlink_color),
                );
            })
            .body(|ui| {
                if let Ok(entries) = std::fs::read_dir(path) {
                    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
                    entries.sort_by_key(|e| {
                        (
                            e.file_type().map(|t| !t.is_dir()).unwrap_or(true),
                            e.file_name(),
                        )
                    });

                    for entry in entries {
                        if let Some(p) = self.render_tree(ui, &entry.path()) {
                            selected = Some(p);
                        }
                    }
                }
            });
        } else {
            let is_selected = self
                .selected_path
                .as_ref()
                .map(|p| p == path)
                .unwrap_or(false);
            let text_color = if is_selected {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().text_color()
            };
            let resp = ui.selectable_label(
                is_selected,
                egui::RichText::new(format!("📄 {}", file_name)).color(text_color),
            );
            if resp.clicked() {
                selected = Some(path.to_path_buf());
            }
            if is_selected {
                resp.scroll_to_me(Some(egui::Align::Center));
            }
        }
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clear_symbol_cache() {
        let mut sidebar = Sidebar::new();

        // Simulate some cached data
        sidebar.symbol_children_cache.insert(NodeId(1), vec![]);
        sidebar.children_loaded.insert(NodeId(1));
        sidebar.symbol_children_cache.insert(NodeId(2), vec![]);
        sidebar.children_loaded.insert(NodeId(2));

        assert_eq!(sidebar.symbol_children_cache.len(), 2);
        assert_eq!(sidebar.children_loaded.len(), 2);

        // Clear cache
        sidebar.clear_symbol_cache();

        assert!(sidebar.symbol_children_cache.is_empty());
        assert!(sidebar.children_loaded.is_empty());
    }

    #[test]
    fn test_cache_initialization() {
        let sidebar = Sidebar::new();

        assert!(sidebar.symbol_children_cache.is_empty());
        assert!(sidebar.children_loaded.is_empty());
        assert_eq!(sidebar.active_tab, SidebarTab::Files);
    }
}
