use codestory_core::{EdgeKind, LayoutDirection, TrailDirection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub theme: ThemeMode,
    pub ui_scale: f32,
    pub font_size: f32,
    #[serde(default = "default_ui_font_family")]
    pub ui_font_family: UiFontFamily,
    #[serde(default = "default_phosphor_variant")]
    pub phosphor_variant: PhosphorVariant,
    pub show_tooltips: bool,
    pub auto_save_interval_secs: u64,
    // Theme customization
    #[serde(default = "default_show_icons")]
    pub show_icons: bool,
    #[serde(default)]
    pub compact_mode: bool,
    #[serde(default = "default_animation_speed")]
    pub animation_speed: f32,

    #[serde(default = "default_auto_open_last_project")]
    pub auto_open_last_project: bool,
    #[serde(default)]
    pub last_opened_project: Option<std::path::PathBuf>,

    #[serde(default)]
    pub recent_projects: Vec<std::path::PathBuf>,

    #[serde(default)]
    pub notifications: NotificationSettings,
    #[serde(default)]
    pub file_dialog: FileDialogSettings,
    #[serde(default)]
    pub node_graph: NodeGraphSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationSettings {
    pub enabled: bool,
    pub position: NotificationPosition,
    pub show_indexing_progress: bool,
    pub show_search_results: bool,
    pub show_file_operations: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NotificationPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            position: NotificationPosition::TopRight,
            show_indexing_progress: true,
            show_search_results: true,
            show_file_operations: true,
        }
    }
}

fn default_show_icons() -> bool {
    true
}
fn default_animation_speed() -> f32 {
    1.0
}
fn default_auto_open_last_project() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    #[serde(alias = "Light")]
    Bright,
    #[serde(alias = "Dark")]
    Dark,
    Latte,
    Frappe,
    Macchiato,
    Mocha,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Bright,
            ui_scale: 1.0,
            font_size: 14.0,
            ui_font_family: UiFontFamily::SourceCodePro,
            phosphor_variant: PhosphorVariant::Regular,
            show_tooltips: true,
            auto_save_interval_secs: 30,
            show_icons: true,
            compact_mode: false,
            animation_speed: 1.0,
            auto_open_last_project: true,
            last_opened_project: None,
            recent_projects: Vec::new(),
            notifications: NotificationSettings::default(),
            file_dialog: FileDialogSettings::default(),
            node_graph: NodeGraphSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiFontFamily {
    SourceCodePro,
    FiraSans,
    Roboto,
}

fn default_ui_font_family() -> UiFontFamily {
    UiFontFamily::SourceCodePro
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhosphorVariant {
    Regular,
    Bold,
    Fill,
    Light,
    Thin,
}

fn default_phosphor_variant() -> PhosphorVariant {
    PhosphorVariant::Regular
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GraphWheelBehavior {
    /// Mouse wheel pans/scrolls the graph; Ctrl/Cmd + wheel zooms.
    ScrollPan,
    /// Mouse wheel zooms the graph.
    Zoom,
}

fn default_graph_wheel_behavior() -> GraphWheelBehavior {
    GraphWheelBehavior::ScrollPan
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeGraphSettings {
    /// Trail depth when querying the graph subview from storage.
    ///
    /// `0` means "infinite" depth, bounded by `TrailConfig.max_nodes` and other safety limits.
    #[serde(default = "default_trail_depth", alias = "max_depth")]
    pub trail_depth: u32,
    #[serde(default = "default_trail_direction")]
    pub trail_direction: TrailDirection,
    #[serde(default)]
    pub trail_edge_filter: Vec<EdgeKind>,
    pub auto_layout: bool,
    pub show_connection_labels: bool,
    pub group_by_file: bool,
    pub group_by_namespace: bool,
    #[serde(default = "default_lod_points_zoom")]
    pub lod_points_zoom: f32,
    #[serde(default = "default_lod_simplified_zoom")]
    pub lod_simplified_zoom: f32,
    #[serde(default = "default_max_full_nodes")]
    pub max_full_nodes: usize,
    #[serde(default)]
    pub show_graph_stats: bool,
    #[serde(default)]
    pub layout_algorithm: codestory_events::LayoutAlgorithm,
    #[serde(default)]
    pub layout_direction: LayoutDirection,

    #[serde(default = "default_true")]
    pub show_classes: bool,
    #[serde(default = "default_true")]
    pub show_functions: bool,
    #[serde(default = "default_true")]
    pub show_variables: bool,
    #[serde(default = "default_true")]
    pub show_minimap: bool,
    #[serde(default)]
    pub show_legend: bool,
    #[serde(default = "default_graph_wheel_behavior")]
    pub graph_wheel_behavior: GraphWheelBehavior,
    #[serde(default = "default_view_state")]
    pub view_state: codestory_graph::uml_types::GraphViewState,
}

fn default_view_state() -> codestory_graph::uml_types::GraphViewState {
    codestory_graph::uml_types::GraphViewState::new()
}

fn default_true() -> bool {
    true
}

fn default_trail_depth() -> u32 {
    1
}

fn default_trail_direction() -> TrailDirection {
    TrailDirection::Both
}

fn default_lod_points_zoom() -> f32 {
    0.4
}

fn default_lod_simplified_zoom() -> f32 {
    0.8
}

fn default_max_full_nodes() -> usize {
    800
}

// LayoutAlgorithm moved to codestory-events

impl Default for NodeGraphSettings {
    fn default() -> Self {
        Self {
            trail_depth: default_trail_depth(),
            trail_direction: TrailDirection::Both,
            trail_edge_filter: Vec::new(),
            auto_layout: true,
            show_connection_labels: true,
            group_by_file: true,
            group_by_namespace: true,
            lod_points_zoom: default_lod_points_zoom(),
            lod_simplified_zoom: default_lod_simplified_zoom(),
            max_full_nodes: default_max_full_nodes(),
            show_graph_stats: false,
            layout_algorithm: codestory_events::LayoutAlgorithm::default(),
            layout_direction: LayoutDirection::default(),
            show_classes: true,
            show_functions: true,
            show_variables: true,
            show_minimap: true,
            show_legend: false,
            graph_wheel_behavior: default_graph_wheel_behavior(),
            view_state: codestory_graph::uml_types::GraphViewState::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_trail_depth_alias_from_max_depth() {
        let value = json!({
            "max_depth": 3,
            "auto_layout": true,
            "show_connection_labels": true,
            "group_by_file": true,
            "group_by_namespace": true
        });

        let settings: NodeGraphSettings =
            serde_json::from_value(value).expect("NodeGraphSettings should deserialize");

        assert_eq!(settings.trail_depth, 3);
    }

    #[test]
    fn test_infinite_trail_depth_roundtrip() {
        let mut settings = NodeGraphSettings::default();
        settings.trail_depth = 0;

        let serialized = serde_json::to_string(&settings).expect("serialize settings");
        let deserialized: NodeGraphSettings =
            serde_json::from_str(&serialized).expect("deserialize settings");

        assert_eq!(deserialized.trail_depth, 0);
    }
}

impl AppSettings {
    pub fn load() -> Self {
        if let Some(config_dir) = dirs::config_dir() {
            let path = config_dir.join("codestory").join("settings.json");
            tracing::info!("Loading settings from {:?}", path);
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(content) => match serde_json::from_str(&content) {
                        Ok(settings) => {
                            tracing::info!("Settings loaded successfully: {:?}", settings);
                            return settings;
                        }
                        Err(e) => tracing::error!("Failed to parse settings: {}", e),
                    },
                    Err(e) => tracing::error!("Failed to read settings file: {}", e),
                }
            } else {
                tracing::info!("Settings file not found, using defaults");
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        if let Some(config_dir) = dirs::config_dir() {
            let app_dir = config_dir.join("codestory");
            if !app_dir.exists() {
                let _ = std::fs::create_dir_all(&app_dir);
            }
            let path = app_dir.join("settings.json");
            if let Ok(content) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(path, content);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDialogSettings {
    pub use_custom_dialogs: bool,
    pub show_hidden_files: bool,
    pub default_width: f32,
    pub default_height: f32,
}

impl Default for FileDialogSettings {
    fn default() -> Self {
        Self {
            use_custom_dialogs: true,
            show_hidden_files: false,
            default_width: 700.0,
            default_height: 500.0,
        }
    }
}
