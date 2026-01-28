//! Dock State Management
//!
//! Manages the docking layout using `egui_dock`. Replaces the custom `LayoutManager`
//! to provide professional IDE-like panel management with drag-and-drop, split panes,
//! and tab reordering.

use egui_dock::{DockState as EguiDockState, NodeIndex};
use serde::{Deserialize, Serialize};

/// Tab identifier for CodeStory panels.
///
/// Each variant represents a distinct panel that can be opened as a tab
/// within the docking system.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TabId {
    /// Code viewer panel - displays source code with syntax highlighting
    Code,
    /// Graph visualization panel - displays code structure as a graph
    Graph,
    /// Detail panel - shows detailed information about selected nodes
    Details,
    /// Error panel - displays indexing errors and warnings
    Errors,
    /// Bookmark panel - shows user bookmarks
    Bookmarks,
    /// Metrics panel - displays codebase analytics (Phase 4)
    Metrics,
    /// Project tree panel - file browser
    ProjectTree,
    /// Trail controls panel - navigation history
    TrailControls,
    /// Overview panel - project overview
    Overview,
    /// Snippet view - merged code snippets from multiple files
    Snippets,
}

impl TabId {
    /// Get the display title for this tab.
    pub fn title(&self) -> &'static str {
        match self {
            TabId::Code => "Code",
            TabId::Graph => "Graph",
            TabId::Details => "Details",
            TabId::Errors => "Errors",
            TabId::Bookmarks => "Bookmarks",
            TabId::Metrics => "Metrics",
            TabId::ProjectTree => "Project",
            TabId::TrailControls => "Trail",
            TabId::Overview => "Overview",
            TabId::Snippets => "Snippets",
        }
    }

    /// Get the icon for this tab.
    pub fn icon(&self) -> &'static str {
        match self {
            TabId::Code => "📄",
            TabId::Graph => "🔀",
            TabId::Details => "ℹ️",
            TabId::Errors => "⚠️",
            TabId::Bookmarks => "🔖",
            TabId::Metrics => "📊",
            TabId::ProjectTree => "📁",
            TabId::TrailControls => "🛤️",
            TabId::Overview => "👁️",
            TabId::Snippets => "📑",
        }
    }

    /// Get combined icon and title for display.
    pub fn display_title(&self) -> String {
        format!("{} {}", self.icon(), self.title())
    }
}

/// Serializable dock state data for persistence.
///
/// Since `egui_dock::DockState` doesn't directly support serde,
/// we store our own representation that can be converted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockStateData {
    /// List of open tabs in order
    pub open_tabs: Vec<TabId>,
    /// Version for migration support
    pub version: u32,
}

impl Default for DockStateData {
    fn default() -> Self {
        Self {
            open_tabs: vec![
                TabId::Code,
                TabId::Graph,
                TabId::Details,
                TabId::Errors,
                TabId::Bookmarks,
            ],
            version: 1,
        }
    }
}

/// Dock state wrapper that manages the egui_dock tree and provides
/// persistence and convenience methods.
pub struct DockState {
    /// The underlying egui_dock state
    dock_state: EguiDockState<TabId>,
    /// Track which tabs are currently open
    open_tabs: Vec<TabId>,
    /// Whether the state has changed since last save
    dirty: bool,
}

impl DockState {
    /// Create a new dock state with the default layout.
    ///
    /// Default layout:
    /// ```text
    /// +----------------+-------------------+
    /// |                |                   |
    /// |     Graph      |      Code         |
    /// |                |                   |
    /// +----------------+-------------------+
    /// |  Errors | Bookmarks | Details      |
    /// +----------------------------------------+
    /// ```
    pub fn new() -> Self {
        // Start with Code as the main tab
        let mut dock_state = EguiDockState::new(vec![TabId::Code]);

        // Get the main surface to manipulate the layout
        let surface = dock_state.main_surface_mut();

        // Split left for Graph (40% width)
        let [_code_node, _left_node] =
            surface.split_left(NodeIndex::root(), 0.40, vec![TabId::Graph]);

        // Split bottom for the bottom panel (25% height)
        surface.split_below(
            NodeIndex::root(),
            0.75,
            vec![
                TabId::Errors,
                TabId::Bookmarks,
                TabId::Details,
                TabId::Metrics,
            ],
        );

        Self {
            dock_state,
            open_tabs: vec![
                TabId::Code,
                TabId::Graph,
                TabId::Errors,
                TabId::Bookmarks,
                TabId::Details,
                TabId::Metrics,
            ],
            dirty: false,
        }
    }

    /// Get mutable reference to the underlying dock state for rendering.
    pub fn dock_state_mut(&mut self) -> &mut EguiDockState<TabId> {
        self.dirty = true;
        &mut self.dock_state
    }

    /// Add a tab to the focused leaf if not already open.
    pub fn add_tab(&mut self, tab: TabId) {
        if !self.open_tabs.contains(&tab) {
            self.dock_state.push_to_focused_leaf(tab.clone());
            self.open_tabs.push(tab);
            self.dirty = true;
        }
    }

    /// Focus on a specific tab (switch to it if open, add if not).
    pub fn focus_tab(&mut self, tab: TabId) {
        if !self.open_tabs.contains(&tab) {
            self.add_tab(tab.clone());
        }
        // Find and focus the tab
        if let Some((surface_index, node_index, _tab_index)) = self.dock_state.find_tab(&tab) {
            self.dock_state
                .set_focused_node_and_surface((surface_index, node_index));
        }
        self.dirty = true;
    }

    /// Check if a tab is currently open.
    pub fn is_tab_open(&self, tab: &TabId) -> bool {
        self.open_tabs.contains(tab)
    }

    /// Get list of open tabs.
    pub fn open_tabs(&self) -> &[TabId] {
        &self.open_tabs
    }

    /// Save dock state to JSON string.
    pub fn save(&self) -> Result<String, serde_json::Error> {
        let data = DockStateData {
            open_tabs: self.open_tabs.clone(),
            version: 1,
        };
        serde_json::to_string_pretty(&data)
    }

    /// Load dock state from JSON string.
    ///
    /// If loading fails or data is from an incompatible version,
    /// returns the default layout.
    pub fn load(json: &str) -> Result<Self, serde_json::Error> {
        let data: DockStateData = serde_json::from_str(json)?;

        // For now, just use the open tabs list to know what should be visible
        // but create a fresh default layout. Full tree serialization would
        // require more complex handling.
        let mut state = Self::new();

        // Add any additional tabs that were open
        for tab in data.open_tabs {
            if !state.open_tabs.contains(&tab) {
                state.add_tab(tab);
            }
        }

        Ok(state)
    }

    /// Get the default file path for saving dock state.
    pub fn default_path() -> std::path::PathBuf {
        if let Some(config_dir) = dirs::config_dir() {
            config_dir.join("codestory").join("dock_layout.json")
        } else {
            std::path::PathBuf::from("dock_layout.json")
        }
    }

    /// Save to the default location.
    pub fn save_default(&self) -> Result<(), std::io::Error> {
        let path = Self::default_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = self
            .save()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        std::fs::write(path, json)
    }

    /// Load from the default location, or create new if not found.
    pub fn load_or_default() -> Self {
        let path = Self::default_path();
        if path.exists()
            && let Ok(json) = std::fs::read_to_string(&path)
            && let Ok(state) = Self::load(&json)
        {
            return state;
        }
        Self::new()
    }
}

impl Default for DockState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_layout() {
        let state = DockState::new();
        assert!(state.is_tab_open(&TabId::Code));
        assert!(state.is_tab_open(&TabId::Graph));
        assert!(state.is_tab_open(&TabId::Errors));
    }

    #[test]
    fn test_add_tab() {
        let mut state = DockState::new();
        assert!(!state.is_tab_open(&TabId::Metrics));

        state.add_tab(TabId::Metrics);
        assert!(state.is_tab_open(&TabId::Metrics));

        // Adding again should be a no-op
        let count_before = state.open_tabs.len();
        state.add_tab(TabId::Metrics);
        assert_eq!(state.open_tabs.len(), count_before);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let state = DockState::new();
        let json = state.save().expect("save should succeed");
        let loaded = DockState::load(&json).expect("load should succeed");

        // Verify same tabs are open
        for tab in state.open_tabs() {
            assert!(loaded.is_tab_open(tab));
        }
    }

    #[test]
    fn test_tab_display() {
        assert_eq!(TabId::Code.title(), "Code");
        assert_eq!(TabId::Code.icon(), "📄");
        assert!(TabId::Code.display_title().contains("Code"));
    }
}
