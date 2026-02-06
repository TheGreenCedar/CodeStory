use codestory_core::{NodeId, NodeKind, SourceLocation};
use codestory_search::SearchEngine;
use codestory_storage::Storage;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::components::{
    bookmark_panel::BookmarkPanel,
    code_view::CodeViewMode,
    code_view::enhanced::EnhancedCodeView,
    code_view::multi_file::MultiFileCodeView,
    commands::{AppState, CommandHistory},
    controller::ActivationController,
    custom_trail_dialog::CustomTrailDialog,
    detail_panel::DetailPanel,
    error_panel::ErrorPanel,
    file_dialog::{DialogResult, FileDialogManager, FileDialogPresets},
    file_watcher::FileWatcher,
    metrics_panel::MetricsPanel,
    node_graph::NodeGraphView,
    notifications::NotificationManager,
    overview::ProjectOverview,
    preferences::PreferencesDialog,
    project_wizard::ProjectWizard,
    recent_files::RecentFiles,
    reference_list::ReferenceList,
    search_bar::{SearchAction, SearchBar, SearchMatch},
    sidebar::Sidebar,
    status_bar::StatusBar,
    tooltip::TooltipManager,
    trail_view::TrailViewControls,
    welcome::{WelcomeAction, WelcomeScreen},
};
use crate::ide_server::IdeServer;
// use crate::dock_state::TabId;
use crate::dock_state::DockState;
use crate::dock_state::TabId;
use crate::navigation::TabManager;
use crate::settings::AppSettings;
use crate::tab_viewer::CodeStoryTabViewer;
use crate::theme::Theme;
use codestory_events::EventListener;
use codestory_events::{ActivationOrigin, Event, EventBus};
use egui_dock::DockArea;
use egui_phosphor::regular as ph;

struct SearchResultItem {
    id: NodeId,
    label: String,
}

pub struct CodeStoryApp {
    storage: Option<Storage>,
    search_engine: Option<SearchEngine>,
    status_message: String,
    node_count: i64,

    // Project Management
    project_path: Option<std::path::PathBuf>,
    show_welcome: bool,
    welcome_screen: WelcomeScreen,

    // Events
    event_bus: EventBus,

    // IDE Integration
    ide_server: IdeServer,

    // Navigation
    tab_manager: TabManager,

    // Settings
    settings: AppSettings,
    preferences_dialog: PreferencesDialog,
    status_bar: StatusBar,
    tooltip_manager: TooltipManager,

    // Async Indexing
    is_indexing: bool,

    // Data Cache
    node_names: HashMap<NodeId, String>,
    search_results: Vec<SearchResultItem>,

    // Components
    search_bar: SearchBar,
    sidebar: Sidebar,
    code_view: EnhancedCodeView,
    code_view_mode: CodeViewMode,
    snippet_view: MultiFileCodeView,
    node_graph_view: NodeGraphView,
    detail_panel: DetailPanel,
    reference_list: ReferenceList,
    error_panel: ErrorPanel,
    bookmark_panel: BookmarkPanel,
    trail_controls: TrailViewControls,
    custom_trail_dialog: CustomTrailDialog,
    project_wizard: ProjectWizard,
    project_overview: ProjectOverview,
    command_history: CommandHistory,
    file_watcher: Option<FileWatcher>,
    metrics_panel: MetricsPanel,
    activation_controller: ActivationController,

    // Layout & Theme
    dock_state: DockState,

    theme: Theme,

    // UI State
    show_project_wizard: bool,
    show_trail_view: bool,
    notification_manager: NotificationManager,
    file_dialog: FileDialogManager,
    recent_files: RecentFiles,

    // Initialization flag - ensures theme is applied on first update() frame
    needs_initial_theme_apply: bool,
    auto_open_attempted: bool,
    last_settings_save: Instant,
}

impl CodeStoryApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let settings = AppSettings::load();

        let mut theme = Theme::new(settings.theme);
        tracing::info!(
            "Applying initial theme mode: {:?} with scale {} and font {}",
            settings.theme,
            settings.ui_scale,
            settings.font_size
        );
        theme.font_size_base = settings.font_size;
        theme.font_size_small = settings.font_size * 0.85;
        theme.font_size_heading = settings.font_size * 1.25;
        theme.show_icons = settings.show_icons;
        theme.compact_mode = settings.compact_mode;
        theme.animation_speed = settings.animation_speed;
        theme.ui_font_family = settings.ui_font_family;
        theme.phosphor_variant = settings.phosphor_variant;

        theme.apply(&_cc.egui_ctx);
        tracing::info!("Theme applied to context");

        // Apply initial UI scale
        _cc.egui_ctx.set_pixels_per_point(settings.ui_scale);

        let mut welcome_screen = WelcomeScreen::new();
        welcome_screen.recent_projects = settings.recent_projects.clone();

        let event_bus = EventBus::new();
        let ide_server = IdeServer::new(event_bus.clone());
        ide_server.start();

        // Load dock state
        let dock_state = DockState::load_or_default();

        Self {
            storage: None,
            search_engine: SearchEngine::new(None).ok(),
            status_message: "Please open a project.".to_string(),
            node_count: 0,
            project_path: None,
            show_welcome: true,
            welcome_screen,
            is_indexing: false,
            node_names: HashMap::new(),
            search_results: Vec::new(),
            search_bar: SearchBar::new(),
            sidebar: Sidebar::new(),
            code_view: {
                let mut cv = EnhancedCodeView::new();
                cv.event_bus = Some(event_bus.clone());
                cv
            },
            code_view_mode: CodeViewMode::SingleFile,
            snippet_view: MultiFileCodeView::new(),
            node_graph_view: NodeGraphView::new(event_bus.clone()),
            detail_panel: DetailPanel::new(),
            reference_list: ReferenceList::new(),
            error_panel: ErrorPanel::new(event_bus.clone()),
            bookmark_panel: BookmarkPanel::new(event_bus.clone()),
            trail_controls: TrailViewControls::new(event_bus.clone()),
            custom_trail_dialog: CustomTrailDialog::new(),
            project_wizard: ProjectWizard::new(),
            project_overview: ProjectOverview::new("No Project".to_string()),
            command_history: CommandHistory::new(100, event_bus.clone()),
            file_watcher: None,
            event_bus: event_bus.clone(),
            ide_server,
            tab_manager: TabManager::new(),
            preferences_dialog: PreferencesDialog::new(&settings),
            status_bar: StatusBar::new(),
            tooltip_manager: TooltipManager::new(),
            settings,
            dock_state,
            theme,
            show_project_wizard: false,
            show_trail_view: false,
            notification_manager: NotificationManager::new(),
            file_dialog: FileDialogManager::new(),
            recent_files: RecentFiles::load(),
            metrics_panel: MetricsPanel::new(),
            activation_controller: ActivationController::new(event_bus),
            needs_initial_theme_apply: true,
            auto_open_attempted: false,
            last_settings_save: Instant::now(),
        }
    }

    fn open_project(&mut self, path: std::path::PathBuf) {
        let storage_path = path.join("codestory.db");
        self.storage = Storage::open(storage_path.to_str().unwrap_or("codestory.db")).ok();
        self.project_path = Some(path.clone());
        self.sidebar.set_root(path.clone());

        // Clear symbol cache for new project
        self.sidebar.clear_symbol_cache();

        // Update recent projects
        self.recent_files.add(path.clone());
        let _ = self.recent_files.save();

        self.settings.last_opened_project = Some(path.clone());
        self.settings.recent_projects = self
            .recent_files
            .get_recent()
            .iter()
            .map(|e| e.path.clone())
            .collect();
        self.settings.save();

        self.welcome_screen.recent_projects = self.settings.recent_projects.clone();

        self.show_welcome = false;
        self.status_message = "Project loaded.".to_string();
        self.event_bus.publish(Event::ProjectOpened {
            path: path.to_string_lossy().to_string(),
        });

        self.project_overview.project_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unknown Project".to_string());

        if let Some(storage) = &self.storage
            && let Ok(stats) = storage.get_stats()
        {
            self.project_overview.set_stats(stats);
        }

        // Initialize FileWatcher
        match FileWatcher::new(self.event_bus.clone()) {
            Ok(mut watcher) => {
                watcher.add_ignore_pattern(".git".to_string());
                watcher.add_ignore_pattern("target".to_string());
                watcher.add_ignore_pattern("node_modules".to_string());
                if let Err(e) = watcher.watch(&path) {
                    tracing::warn!("Failed to watch project directory: {}", e);
                }
                self.file_watcher = Some(watcher);
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to initialize file watcher, file change detection disabled: {}",
                    e
                );
            }
        }

        // Update Recent Projects
        if !self.settings.recent_projects.contains(&path) {
            self.settings.recent_projects.insert(0, path.clone());
            if self.settings.recent_projects.len() > 10 {
                self.settings.recent_projects.truncate(10);
            }
            self.settings.save();
            self.welcome_screen.recent_projects = self.settings.recent_projects.clone();
        } else {
            // Move to top
            if let Some(pos) = self
                .settings
                .recent_projects
                .iter()
                .position(|p| p == &path)
            {
                self.settings.recent_projects.remove(pos);
                self.settings.recent_projects.insert(0, path.clone());
                self.settings.save();
                self.welcome_screen.recent_projects = self.settings.recent_projects.clone();
            }
        }

        // Load tab state if exists
        let state_path = path.join("codestory_ui.json");
        if state_path.exists()
            && let Ok(data) = std::fs::read_to_string(&state_path)
            && let Ok(tabs) = serde_json::from_str::<TabManager>(&data)
        {
            self.tab_manager = tabs;
        }

        // Initial load and indexing
        if let Some(storage) = &self.storage {
            if let Ok(nodes) = storage.get_nodes() {
                self.node_count = nodes.len() as i64;
                self.node_names.clear();
                let mut search_nodes = Vec::new();
                for node in nodes {
                    let display_name = node
                        .qualified_name
                        .clone()
                        .unwrap_or_else(|| node.serialized_name.clone());
                    self.node_names.insert(node.id, display_name.clone());
                    search_nodes.push((node.id, display_name));
                }
                if let Some(engine) = &mut self.search_engine {
                    let _ = engine.index_nodes(search_nodes);
                }
            }

            // Load bookmarks
            if let Ok(categories) = storage.get_bookmark_categories()
                && let Ok(bookmarks) = storage.get_bookmarks(None)
            {
                self.bookmark_panel.set_data(categories, bookmarks);
            }

            // Compute metrics after project is loaded
            let metrics =
                crate::components::metrics_panel::CodebaseMetrics::compute_from_storage(storage);
            self.metrics_panel.set_metrics(metrics);
        }

        // Restore active node from tab state - MUST be outside storage borrow
        if let Some(tab) = self.tab_manager.active_tab()
            && let Some(node_id) = tab.active_node
        {
            self.select_node(node_id);
        }
    }

    fn node_kind_label(kind: NodeKind) -> &'static str {
        match kind {
            NodeKind::MODULE => "mod",
            NodeKind::NAMESPACE => "ns",
            NodeKind::PACKAGE => "pkg",
            NodeKind::FILE => "file",
            NodeKind::STRUCT => "struct",
            NodeKind::CLASS => "class",
            NodeKind::INTERFACE => "iface",
            NodeKind::ANNOTATION => "anno",
            NodeKind::UNION => "union",
            NodeKind::ENUM => "enum",
            NodeKind::TYPEDEF => "typedef",
            NodeKind::TYPE_PARAMETER => "typeparam",
            NodeKind::BUILTIN_TYPE => "builtin",
            NodeKind::FUNCTION => "fn",
            NodeKind::METHOD => "method",
            NodeKind::MACRO => "macro",
            NodeKind::GLOBAL_VARIABLE => "gvar",
            NodeKind::FIELD => "field",
            NodeKind::VARIABLE => "var",
            NodeKind::CONSTANT => "const",
            NodeKind::ENUM_CONSTANT => "enumconst",
            NodeKind::UNKNOWN => "sym",
        }
    }

    fn build_search_results(&self, ids: Vec<NodeId>) -> Vec<SearchResultItem> {
        ids.into_iter()
            .map(|id| {
                let name = self
                    .node_names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| id.0.to_string());

                let mut kind_label = "sym";
                let mut file_label = String::new();

                if let Some(storage) = &self.storage {
                    if let Ok(Some(node)) = storage.get_node(id) {
                        kind_label = Self::node_kind_label(node.kind);
                    }
                    if let Ok(occs) = storage.get_occurrences_for_node(id)
                        && let Some(occ) = occs.first()
                        && let Ok(Some(file_node)) = storage.get_node(occ.location.file_node_id)
                    {
                        file_label = file_node.serialized_name;
                    }
                }

                let label = if file_label.is_empty() {
                    format!("{} ({})", name, kind_label)
                } else {
                    format!("{} ({}) - {}", name, kind_label, file_label)
                };

                SearchResultItem { id, label }
            })
            .collect()
    }

    fn build_search_matches(&self, ids: Vec<NodeId>) -> Vec<SearchMatch> {
        ids.into_iter()
            .take(10) // Limit autocomplete results
            .map(|id| {
                let name = self
                    .node_names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| id.0.to_string());

                let mut kind_str = "symbol".to_string();
                let mut file_path = None;
                let mut line = None;

                if let Some(storage) = &self.storage {
                    if let Ok(Some(node)) = storage.get_node(id) {
                        kind_str = Self::node_kind_label(node.kind).to_string();
                    }
                    if let Ok(occs) = storage.get_occurrences_for_node(id)
                        && let Some(occ) = occs.first()
                    {
                        if let Ok(Some(file_node)) = storage.get_node(occ.location.file_node_id) {
                            file_path = Some(file_node.serialized_name);
                        }
                        line = Some(occ.location.start_line);
                    }
                }

                SearchMatch {
                    node_id: id,
                    name: name.clone(),
                    qualified_name: name, // TODO: fetch qualified name from storage
                    kind: kind_str,
                    file_path,
                    line,
                    score: 1.0, // TODO: preserve score from search
                }
            })
            .collect()
    }

    fn start_indexing(&mut self) {
        if self.is_indexing {
            return;
        }

        let project_path = match &self.project_path {
            Some(p) => p.clone(),
            None => return,
        };

        // Clear symbol cache since we're reindexing
        self.sidebar.clear_symbol_cache();

        self.is_indexing = true;
        self.status_message = "Indexing...".to_string();

        let root = project_path.to_str().unwrap_or(".").to_string();
        let storage_path = project_path
            .join("codestory.db")
            .to_str()
            .unwrap_or("codestory.db")
            .to_string();

        let event_bus = self.event_bus.clone();
        let start_time = std::time::Instant::now();

        std::thread::spawn(move || {
            if let Ok(mut storage) = Storage::open(&storage_path) {
                if let Err(e) = storage.clear() {
                    event_bus.publish(Event::IndexingFailed {
                        error: format!("Error clearing storage: {}", e),
                    });
                    return;
                }

                // Use the Project API
                let project =
                    match codestory_project::Project::open(std::path::PathBuf::from(&root)) {
                        Ok(p) => p,
                        Err(e) => {
                            event_bus.publish(Event::IndexingFailed {
                                error: format!("Error opening project: {}", e),
                            });
                            return;
                        }
                    };

                let refresh_info = match project.full_refresh() {
                    Ok(info) => info,
                    Err(e) => {
                        event_bus.publish(Event::IndexingFailed {
                            error: format!("Error getting files: {}", e),
                        });
                        return;
                    }
                };

                let total_files = refresh_info.files_to_index.len();
                event_bus.publish(Event::IndexingStarted {
                    file_count: total_files,
                });

                // Use the new WorkspaceIndexer API
                let indexer =
                    codestory_index::WorkspaceIndexer::new(std::path::PathBuf::from(&root));

                // Run indexing
                let result = indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None);

                if let Err(e) = result {
                    event_bus.publish(Event::IndexingFailed {
                        error: format!("Error: {}", e),
                    });
                } else {
                    event_bus.publish(Event::IndexingComplete {
                        duration_ms: start_time.elapsed().as_millis() as u64,
                    });
                }
            } else {
                event_bus.publish(Event::IndexingFailed {
                    error: "Error: Could not open storage".to_string(),
                });
            }
        });
    }

    fn select_node_graph_detail(&mut self, id: NodeId) {
        if let Some(storage) = &self.storage {
            // Update Graph using trail with current depth
            let trail_config = codestory_core::TrailConfig {
                root_id: id,
                depth: self.settings.node_graph.trail_depth,
                direction: self.settings.node_graph.trail_direction,
                edge_filter: self.settings.node_graph.trail_edge_filter.clone(),
                max_nodes: 500,
            };

            match storage.get_trail(&trail_config) {
                Ok(result) => {
                    self.node_graph_view.load_from_data(
                        id,
                        &result.nodes,
                        &result.edges,
                        &self.settings.node_graph,
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to get trail for neighborhood: {}", e);
                    // Fallback to simple neighborhood
                    if let Ok((nodes, edges)) = storage.get_neighborhood(id) {
                        self.node_graph_view.load_from_data(
                            id,
                            &nodes,
                            &edges,
                            &self.settings.node_graph,
                        );
                    }
                }
            }

            // Update Detail Panel (always showing direct neighborhood for clarity in details)
            match storage.get_node(id) {
                Ok(Some(node)) => match storage.get_neighborhood(id) {
                    Ok((_, edges)) => {
                        self.detail_panel.set_data(node, edges);
                    }
                    Err(e) => {
                        tracing::error!("Failed to get edges for detail: {}", e);
                    }
                },
                Ok(None) => tracing::warn!("Selected node not found: {}", id.0),
                Err(e) => tracing::error!("Error fetching selected node: {}", e),
            }
        }
    }

    fn select_node(&mut self, id: NodeId) {
        self.select_node_graph_detail(id);

        if let Some(storage) = &self.storage {
            // Update Code View
            match storage.get_occurrences_for_node(id) {
                Ok(occs) => {
                    let active_occ = occs
                        .iter()
                        .find(|occ| matches!(occ.kind, codestory_core::OccurrenceKind::DEFINITION))
                        .cloned()
                        .or_else(|| occs.first().cloned());
                    let active_file = active_occ.as_ref().map(|occ| occ.location.file_node_id);

                    self.reference_list.set_data(occs.clone(), active_file);

                    if let Some(active_occ) = active_occ.as_ref() {
                        let file_id = active_occ.location.file_node_id;
                        let file_occs: Vec<_> = occs
                            .iter()
                            .filter(|o| o.location.file_node_id == file_id)
                            .cloned()
                            .collect();

                        match storage.get_node(file_id) {
                            Ok(Some(file_node)) => {
                                let path_str = file_node.serialized_name;
                                if std::path::Path::new(&path_str).exists() {
                                    if let Ok(content) = std::fs::read_to_string(&path_str) {
                                        self.code_view.set_file(
                                            path_str,
                                            content,
                                            active_occ.location.start_line as usize,
                                        );

                                        self.code_view.active_locations =
                                            vec![active_occ.location.clone()];
                                        self.code_view.occurrences = file_occs;
                                        self.sidebar.set_selected_path(std::path::PathBuf::from(
                                            &self.code_view.path,
                                        ));
                                    } else {
                                        tracing::error!("Failed to read file: {}", path_str);
                                    }
                                }
                            }
                            Ok(None) => tracing::warn!("File node not found for id {}", file_id),
                            Err(e) => tracing::error!("Error fetching file node: {}", e),
                        }
                    }

                    self.snippet_view.clear();
                    if !occs.is_empty() {
                        let mut file_paths: HashMap<NodeId, std::path::PathBuf> = HashMap::new();
                        for occ in &occs {
                            let file_id = occ.location.file_node_id;
                            if !file_paths.contains_key(&file_id) {
                                if let Ok(Some(file_node)) = storage.get_node(file_id) {
                                    let path =
                                        std::path::PathBuf::from(file_node.serialized_name.clone());
                                    if path.exists()
                                        && let Ok(content) = std::fs::read_to_string(&path)
                                    {
                                        self.snippet_view.add_file(
                                            path.clone(),
                                            content,
                                            Some(file_id),
                                        );
                                        file_paths.insert(file_id, path);
                                    }
                                }
                            }

                            if let Some(path) = file_paths.get(&file_id) {
                                self.snippet_view.add_occurrence(path, occ.clone());
                            }
                        }

                        if let Some(active_occ) = active_occ.as_ref() {
                            self.snippet_view.set_focus(active_occ.location.clone());
                        }

                        if let Ok(errors) = storage.get_errors(None) {
                            let mut counts = HashMap::new();
                            for error in errors {
                                if let Some(file_id) = error.file_id {
                                    *counts.entry(file_id).or_insert(0) += 1;
                                }
                            }
                            self.snippet_view.set_error_counts(counts);
                        }

                        self.snippet_view.sort_files_by_references();
                    }
                }
                Err(e) => tracing::error!("Failed to get occurrences: {}", e),
            }
        }
    }

    fn try_auto_open_last_project(&mut self) {
        if !self.settings.auto_open_last_project {
            return;
        }
        if self.project_path.is_some() {
            return;
        }

        let mut candidate = self.settings.last_opened_project.clone();
        if candidate.is_none() {
            candidate = self.settings.recent_projects.first().cloned();
        }

        if candidate.is_none() {
            return;
        }

        let mut best = None;
        if let Some(path) = candidate {
            if path.exists() && path.join("codestory.db").exists() {
                best = Some(path);
            }
        }

        if best.is_none() {
            for path in &self.settings.recent_projects {
                if path.exists() && path.join("codestory.db").exists() {
                    best = Some(path.clone());
                    break;
                }
            }
        }

        if let Some(path) = best {
            self.open_project(path);
        }
    }
}

impl eframe::App for CodeStoryApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Save tab state
        if let Some(path) = &self.project_path {
            let state_path = path.join("codestory_ui.json");
            if let Ok(data) = serde_json::to_string_pretty(&self.tab_manager) {
                let _ = std::fs::write(state_path, data);
            }
        }
        self.settings.save();

        // Log panel state
        tracing::info!("Exiting with DockState: {:?}", self.dock_state.open_tabs());

        // Save dock state
        let _ = self.dock_state.save_default();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Theme Management - apply on first frame or when flavor changes
        if self.needs_initial_theme_apply || self.theme.mode != self.settings.theme {
            tracing::info!(
                "Applying theme on first frame or mode change: {:?}",
                self.settings.theme
            );
            self.theme = Theme::new(self.settings.theme);
            // Sync all settings to theme
            self.theme.font_size_base = self.settings.font_size;
            self.theme.font_size_small = self.settings.font_size * 0.85;
            self.theme.font_size_heading = self.settings.font_size * 1.25;
            self.theme.show_icons = self.settings.show_icons;
            self.theme.compact_mode = self.settings.compact_mode;
            self.theme.animation_speed = self.settings.animation_speed;
            self.theme.ui_font_family = self.settings.ui_font_family;
            self.theme.apply(ctx);
            ctx.set_pixels_per_point(self.settings.ui_scale);
            self.needs_initial_theme_apply = false;
        }

        if !self.auto_open_attempted {
            self.auto_open_attempted = true;
            self.try_auto_open_last_project();
        }

        // rect tracking removed

        // Handle keyboard shortcuts
        if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft)) {
            self.event_bus.publish(Event::HistoryBack);
        }
        if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowRight)) {
            self.event_bus.publish(Event::HistoryForward);
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::T)) {
            self.event_bus.publish(Event::TabOpen { token_id: None });
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::W)) {
            self.event_bus.publish(Event::TabClose {
                index: self.tab_manager.active_tab_index,
            });
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z)) {
            self.event_bus.publish(Event::Undo);
        }
        if ctx.input(|i| {
            (i.modifiers.command && i.key_pressed(egui::Key::Y))
                || (i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::Z))
        }) {
            self.event_bus.publish(Event::Redo);
        }
        let f3_pressed = ctx.input(|i| i.key_pressed(egui::Key::F3));
        if f3_pressed {
            let reverse = ctx.input(|i| i.modifiers.shift);
            let occ = if reverse {
                self.reference_list.prev_occurrence()
            } else {
                self.reference_list.next_occurrence()
            };
            if let Some(occ) = occ {
                self.event_bus.publish(Event::ShowReference {
                    location: occ.location,
                });
            }
        }
        for i in 0..9 {
            let key = match i {
                0 => egui::Key::Num1,
                1 => egui::Key::Num2,
                2 => egui::Key::Num3,
                3 => egui::Key::Num4,
                4 => egui::Key::Num5,
                5 => egui::Key::Num6,
                6 => egui::Key::Num7,
                7 => egui::Key::Num8,
                8 => egui::Key::Num9,
                _ => continue,
            };
            if ctx.input(|inp| inp.modifiers.command && inp.key_pressed(key)) {
                self.event_bus.publish(Event::TabSelect { index: i });
            }
        }

        // Dispatch pending events
        let rx = self.event_bus.receiver();
        // Check for file dialog results
        self.handle_file_dialog_results();

        while let Ok(event) = rx.try_recv() {
            // self.layout_manager.handle_event(&event); // Removed
            self.handle_event(&event);
            self.handle_notification_event(&event);
        }

        // Sync notification position
        let _anchor = match self.settings.notifications.position {
            crate::settings::NotificationPosition::TopLeft => egui_notify::Anchor::TopLeft,
            crate::settings::NotificationPosition::TopRight => egui_notify::Anchor::TopRight,
            crate::settings::NotificationPosition::BottomLeft => egui_notify::Anchor::BottomLeft,
            crate::settings::NotificationPosition::BottomRight => egui_notify::Anchor::BottomRight,
        };
        // Note: Anchor is set during NotificationManager::new() and cannot be changed dynamically
        // self.notification_manager.set_anchor(anchor);

        // Render Preferences Dialog
        self.preferences_dialog.show(ctx, &mut self.settings);

        // Render file dialog
        self.file_dialog.render(ctx, &self.theme);

        // Notifications last
        self.notification_manager.render(ctx, &self.theme);
        // Dock state check omitted here, using save_default on exit

        // Render Tooltips
        if self.settings.show_tooltips {
            self.tooltip_manager.ui(ctx);
        }

        // Project Wizard
        if self.show_project_wizard
            && let Some(config) = self
                .project_wizard
                .ui(ctx, &mut self.file_dialog, &self.settings)
        {
            tracing::info!("Project Wizard finished with config: {:?}", config);
            // Here we would create the project and open it
            self.show_project_wizard = false;
        }

        // (Panels are now in the DockArea)

        // Custom Trail Dialog
        if self.custom_trail_dialog.is_open
            && self.custom_trail_dialog.ui(
                ctx,
                &self.event_bus,
                self.search_engine.as_mut(),
                self.storage.as_ref(),
            )
        {
            // User clicked "Start Trail"
            if let Some(config) = self.custom_trail_dialog.build_config() {
                // Start the trail
                self.show_trail_view = true;
                if let Some(storage) = &self.storage {
                    match storage.get_trail(&config) {
                        Ok(result) => {
                            self.node_graph_view.load_from_data(
                                config.root_id,
                                &result.nodes,
                                &result.edges,
                                &self.settings.node_graph,
                            );
                        }
                        Err(e) => tracing::error!("Failed to start trail: {}", e),
                    }
                }
                self.custom_trail_dialog.close();
            }
        }

        // Menu Bar
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New Project (Wizard)...").clicked() {
                        self.project_wizard.open();
                        self.show_project_wizard = true;
                        ui.close();
                    }
                    if ui.button("Open Project...").clicked() {
                        FileDialogPresets::open_project(&mut self.file_dialog);
                        self.file_dialog.open_directory(
                            "Open CodeStory Project Folder",
                            "open_project",
                            &self.settings.file_dialog,
                        );
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Preferences").clicked() {
                        self.preferences_dialog.sync_with_current(&self.settings);
                        self.preferences_dialog.open = true;
                        ui.close();
                    }
                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Edit", |ui| {
                    let mut undo_target = None;
                    if ui
                        .add_enabled(self.command_history.can_undo(), egui::Button::new("Undo"))
                        .clicked()
                    {
                        if let Some(tab) = self.tab_manager.active_tab_mut() {
                            let mut state = AppState {
                                active_node: &mut tab.active_node,
                            };
                            if self.command_history.undo(&mut state).is_ok() {
                                undo_target = *state.active_node;
                            }
                        }
                        ui.close();
                    }
                    if let Some(id) = undo_target {
                        self.select_node(id);
                    }

                    let mut redo_target = None;
                    if ui
                        .add_enabled(self.command_history.can_redo(), egui::Button::new("Redo"))
                        .clicked()
                    {
                        if let Some(tab) = self.tab_manager.active_tab_mut() {
                            let mut state = AppState {
                                active_node: &mut tab.active_node,
                            };
                            if self.command_history.redo(&mut state).is_ok() {
                                redo_target = *state.active_node;
                            }
                        }
                        ui.close();
                    }
                    if let Some(id) = redo_target {
                        self.select_node(id);
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui
                        .checkbox(
                            &mut self
                                .dock_state
                                .is_tab_open(&crate::dock_state::TabId::Bookmarks),
                            "Bookmarks",
                        )
                        .clicked()
                    {
                        self.dock_state
                            .focus_tab(crate::dock_state::TabId::Bookmarks);
                    }
                    if ui
                        .checkbox(
                            &mut self
                                .dock_state
                                .is_tab_open(&crate::dock_state::TabId::Errors),
                            "Errors",
                        )
                        .clicked()
                    {
                        self.dock_state.focus_tab(crate::dock_state::TabId::Errors);
                    }
                    if ui
                        .checkbox(
                            &mut self
                                .dock_state
                                .is_tab_open(&crate::dock_state::TabId::Graph),
                            "Graph",
                        )
                        .clicked()
                    {
                        self.dock_state.focus_tab(crate::dock_state::TabId::Graph);
                    }
                    ui.checkbox(&mut self.show_trail_view, "Trail View");

                    // Keep legend visibility in the persisted graph settings.
                    let mut show_graph_legend = self.settings.node_graph.show_legend;
                    if ui
                        .checkbox(&mut show_graph_legend, "Graph Legend")
                        .changed()
                    {
                        self.event_bus
                            .publish(Event::SetShowLegend(show_graph_legend));
                    }
                    ui.separator();

                    if ui.button("Reset Layout").clicked() {
                        self.dock_state = crate::dock_state::DockState::new();
                        ui.close();
                    }
                });
            });
        });

        if self.show_welcome {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(action) =
                    self.welcome_screen
                        .ui(ui, &mut self.file_dialog, &self.settings)
                {
                    match action {
                        WelcomeAction::OpenRecent(path) => {
                            self.open_project(path);
                        }
                    }
                }
            });
            return;
        }

        // Navigation is now handled by events from tabs

        // Top Panel: Search + Tabs
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Navigation Buttons
                ui.group(|ui| {
                    if ui
                        .add_enabled(
                            self.tab_manager
                                .active_tab()
                                .is_some_and(|t| t.history.can_go_back()),
                            egui::Button::new(ph::ARROW_LEFT),
                        )
                        .clicked()
                    {
                        self.event_bus.publish(Event::HistoryBack);
                    }
                    if ui
                        .add_enabled(
                            self.tab_manager
                                .active_tab()
                                .is_some_and(|t| t.history.can_go_forward()),
                            egui::Button::new(ph::ARROW_RIGHT),
                        )
                        .clicked()
                    {
                        self.event_bus.publish(Event::HistoryForward);
                    }

                    let mut jump_to_index = None;
                    if let Some(tab) = self.tab_manager.active_tab_mut() {
                        let history_response = ui.menu_button(ph::CLOCK_COUNTER_CLOCKWISE, |ui| {
                            let entries = tab.history.entries();
                            let current = tab.history.current_index();
                            if entries.is_empty() {
                                ui.label("No history yet");
                                return;
                            }
                            for (idx, entry) in entries.iter().enumerate() {
                                let label = entry
                                    .node_ids
                                    .first()
                                    .and_then(|id| self.node_names.get(id))
                                    .cloned()
                                    .unwrap_or_else(|| "Unknown".to_string());
                                if ui.selectable_label(current == Some(idx), label).clicked() {
                                    jump_to_index = Some(idx);
                                    ui.close();
                                }
                            }
                        });
                        history_response.response.on_hover_text("History");

                        if let Some(index) = jump_to_index {
                            if let Some(entry) = tab.history.jump_to(index)
                                && let Some(node_id) = entry.node_ids.first().cloned()
                            {
                                self.event_bus.publish(Event::ActivateNode {
                                    id: node_id,
                                    origin: ActivationOrigin::Search,
                                });
                            }
                        }
                    }
                });

                ui.separator();

                match self.search_bar.ui(ui) {
                    SearchAction::FullSearch(query) => {
                        if let Some(engine) = &mut self.search_engine {
                            let results = engine.search_symbol(&query);
                            self.search_results = self.build_search_results(results);
                            // Update status message with search info
                            self.status_message = format!(
                                "Found {} results for '{}'",
                                self.search_results.len(),
                                self.search_bar.query()
                            );
                        }
                    }
                    SearchAction::Autocomplete(query) => {
                        // Request autocomplete results
                        if let Some(engine) = &mut self.search_engine {
                            let ids = engine.search_symbol(&query);
                            let suggestions = self.build_search_matches(ids);
                            self.search_bar.set_suggestions(suggestions);
                        }
                    }
                    SearchAction::SelectMatch(match_) => {
                        // Update query to show selected match name
                        self.search_bar.set_query(match_.name.clone());
                        // Navigate to the selected match
                        self.event_bus.publish(Event::ActivateNode {
                            id: match_.node_id,
                            origin: ActivationOrigin::Search,
                        });
                        self.search_bar.clear_suggestions();
                    }
                    SearchAction::None => {}
                }

                ui.separator();
                if ui
                    .button(ph::ARROW_CLOCKWISE)
                    .on_hover_text("Refresh Index")
                    .clicked()
                {
                    self.start_indexing();
                }
                if ui.button(ph::EYE).on_hover_text("Overview").clicked() {
                    self.dock_state
                        .focus_tab(crate::dock_state::TabId::Overview);
                }
            });

            // Tabs UI removed (handled by egui_dock)

            // Basic Results Display (Collapsible)
            if !self.search_results.is_empty() {
                ui.separator();
                ui.collapsing(
                    format!("Search Results ({})", self.search_results.len()),
                    |ui| {
                        let results_width = ui.available_width().max(300.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(results_width, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_min_width(results_width);
                                egui::ScrollArea::both()
                                    .auto_shrink([false, false])
                                    .max_height(200.0)
                                    .show(ui, |ui| {
                                        for result in &self.search_results {
                                            // Truncate long labels to prevent overflow
                                            let display_label = if result.label.chars().count() > 60
                                            {
                                                let truncated: String =
                                                    result.label.chars().take(57).collect();
                                                format!("{}...", truncated)
                                            } else {
                                                result.label.clone()
                                            };

                                            let response =
                                                ui.selectable_label(false, &display_label);
                                            if response.clicked() {
                                                tracing::info!(
                                                    "Selected node via search: {}",
                                                    result.label
                                                );
                                                self.event_bus.publish(Event::ActivateNode {
                                                    id: result.id,
                                                    origin: ActivationOrigin::Search,
                                                });
                                            }
                                            // Show full label on hover if truncated
                                            if display_label != result.label {
                                                response.on_hover_text(&result.label);
                                            }
                                        }
                                    });
                            },
                        );
                    },
                );
            }
        });

        // Bottom Panel: Status (Simplified)
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            let error_count = self
                .storage
                .as_ref()
                .and_then(|s| s.get_stats().ok())
                .map(|s| s.error_count as usize)
                .unwrap_or(0);

            let (start_indexing, toggle_errors) = self.status_bar.ui(
                ui,
                &self.status_message,
                self.node_count,
                error_count,
                self.is_indexing,
            );

            if start_indexing {
                self.start_indexing();
            }
            if toggle_errors {
                self.dock_state.focus_tab(crate::dock_state::TabId::Errors);
            }

            ui.separator();
            ui.label(format!("IDE Server: :{}", self.ide_server.port));
        });

        // Sidebar and Right Panel removed (moved to tabs)

        // Right Panel and Central Panel replaced by DockArea
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut settings_dirty = false;
            let mut tab_viewer = CodeStoryTabViewer {
                theme: &self.theme,
                code_view: &mut self.code_view,
                code_view_mode: &mut self.code_view_mode,
                snippet_view: &mut self.snippet_view,
                detail_panel: &mut self.detail_panel,
                bookmark_panel: &mut self.bookmark_panel,
                overview: &mut self.project_overview,
                trail_controls: &mut self.trail_controls,
                node_names: &self.node_names,
                storage: &self.storage,
                error_panel: &mut self.error_panel,
                sidebar: &mut self.sidebar,
                reference_list: &mut self.reference_list,
                event_bus: &self.event_bus,
                metrics_panel: &mut self.metrics_panel,
                node_graph_view: &mut self.node_graph_view,
                settings: &mut self.settings,
                settings_dirty: &mut settings_dirty,
            };

            DockArea::new(self.dock_state.dock_state_mut())
                .style(crate::theme::dock_style(ctx))
                .show_inside(ui, &mut tab_viewer);

            if settings_dirty && self.last_settings_save.elapsed() >= Duration::from_millis(500) {
                self.settings.save();
                self.last_settings_save = Instant::now();
            }
        });

        // Execute navigation if any removed (handled by events)

        // Render notifications
        self.notification_manager.render(ctx, &self.theme);
    }
}

impl codestory_events::EventListener for CodeStoryApp {
    fn handle_event(&mut self, event: &Event) {
        self.node_graph_view
            .handle_event(event, &self.storage, &self.settings.node_graph);

        match event {
            Event::ActivateNode { id, origin: _ } => {
                let storage = &self.storage;
                let settings = &self.settings;
                let node_names = &self.node_names;

                self.activation_controller.handle_event(
                    event,
                    storage,
                    settings,
                    &mut self.tab_manager,
                    &mut self.code_view,
                    &mut self.snippet_view,
                    &mut self.node_graph_view,
                    &mut self.detail_panel,
                    &mut self.reference_list,
                    &mut self.sidebar,
                    &mut self.command_history,
                    node_names,
                );

                // Async Node Graph Loading (Moved to controller potentially, but kept here for now as it uses project_path)
                if let Some(path) = self.project_path.clone() {
                    let center_id = *id;
                    let event_bus = self.event_bus.clone();
                    std::thread::spawn(move || {
                        let storage_path = path.join("codestory.db");
                        // We need to re-open storage here as we can't easily share the main thread's connection
                        // without Arc/Mutex refactoring. For read-only query this is fine.
                        if let Ok(storage) =
                            Storage::open(storage_path.to_str().unwrap_or("codestory.db"))
                            && let Ok((nodes, edges)) = storage.get_neighborhood(center_id)
                        {
                            event_bus.publish(Event::NeighborhoodLoaded {
                                center_id,
                                nodes,
                                edges,
                            });
                        }
                    });
                }

                // Clear edge focus when a node is selected
                // self.graph_view.set_focused_edge(None); // Legacy

                // Update tab title if it's a new node
                if let Some(name) = self.node_names.get(id)
                    && let Some(tab) = self.tab_manager.active_tab_mut()
                {
                    tab.title = name.clone();
                }
            }
            Event::ActivateEdge { id } => {
                tracing::info!("Event: ActivateEdge {:?}", id);
                // Focus the edge in the node graph view if supported
            }
            Event::HistoryBack => {
                let node_id = if let Some(tab) = self.tab_manager.active_tab_mut() {
                    tab.history.back().and_then(|e| {
                        tracing::debug!("History back to timestamp: {:?}", e.timestamp);
                        e.node_ids.first().cloned()
                    })
                } else {
                    None
                };
                if let Some(id) = node_id {
                    self.select_node(id);
                }
            }
            Event::HistoryForward => {
                let node_id = if let Some(tab) = self.tab_manager.active_tab_mut() {
                    tab.history.forward().and_then(|e| {
                        tracing::debug!("History forward to timestamp: {:?}", e.timestamp);
                        e.node_ids.first().cloned()
                    })
                } else {
                    None
                };
                if let Some(id) = node_id {
                    self.select_node(id);
                }
            }
            Event::TabOpen { token_id } => {
                let title = token_id
                    .and_then(|id| self.node_names.get(&id).cloned())
                    .unwrap_or_else(|| "New Tab".to_string());
                self.tab_manager.open_tab(title, *token_id);
                if let Some(id) = token_id {
                    self.select_node(*id);
                }
            }
            Event::TabClose { index } => {
                self.tab_manager.close_tab(*index);
            }
            Event::TabSelect { index } => {
                self.tab_manager.select_tab(*index);
                if let Some(tab) = self.tab_manager.active_tab()
                    && let Some(id) = tab.active_node
                {
                    self.select_node(id);
                }
            }
            Event::ShowReference { location } => {
                if let Some(storage) = &self.storage {
                    let file_id = location.file_node_id;
                    if let Ok(Some(file_node)) = storage.get_node(file_id) {
                        let path = std::path::PathBuf::from(file_node.serialized_name.clone());
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            self.code_view.show_location(
                                file_node.serialized_name,
                                content,
                                location.clone(),
                            );
                            self.code_view.occurrences =
                                self.reference_list.occurrences_for_file(file_id);
                            self.reference_list.set_active_file(Some(file_id));
                            self.snippet_view.set_focus(location.clone());
                            self.sidebar.set_selected_path(path);
                        }
                    }
                }
            }
            Event::ScrollToLine { file, line } => {
                if let Ok(content) = std::fs::read_to_string(file) {
                    let location = SourceLocation {
                        file_node_id: NodeId(0),
                        start_line: *line as u32,
                        start_col: 0,
                        end_line: *line as u32,
                        end_col: 0,
                    };
                    self.code_view.show_location(
                        file.to_string_lossy().to_string(),
                        content,
                        location,
                    );
                    self.reference_list.set_active_file(None);
                    self.sidebar.set_selected_path(file.clone());
                }
            }
            Event::TooltipShow { info, x, y } => {
                self.tooltip_manager
                    .show(info.clone(), egui::Pos2::new(*x, *y));
            }
            Event::TooltipHide => {
                self.tooltip_manager.hide();
            }
            Event::Undo => {
                if let Some(tab) = self.tab_manager.active_tab_mut() {
                    let mut state = AppState {
                        active_node: &mut tab.active_node,
                    };
                    let _ = self.command_history.undo(&mut state);
                }
            }
            Event::Redo => {
                if let Some(tab) = self.tab_manager.active_tab_mut() {
                    let mut state = AppState {
                        active_node: &mut tab.active_node,
                    };
                    let _ = self.command_history.redo(&mut state);
                }
            }
            Event::FilesChanged { paths } => {
                tracing::info!("Files changed: {:?}", paths);
                self.status_message =
                    format!("{} files changed. Re-indexing suggested.", paths.len());
                // In a real app, we might trigger auto-indexing here
            }
            Event::BookmarkAdd {
                node_id,
                category_id,
            } => {
                if let Some(storage) = &self.storage {
                    if let Err(e) = storage.add_bookmark(*category_id, *node_id, None) {
                        tracing::error!("Failed to add bookmark: {}", e);
                    } else {
                        self.status_message = "Bookmark added.".to_string();
                    }
                }
            }
            Event::BookmarkAddDefault { node_id } => {
                self.bookmark_panel.add_bookmark_to_default(*node_id);
            }
            Event::TrailModeEnter { root_id } => {
                self.show_trail_view = true;
                self.trail_controls.activate_from_event(*root_id);
                if let Some(storage) = &self.storage
                    && let Some(config) = self.trail_controls.config.to_trail_config()
                {
                    match storage.get_trail(&config) {
                        Ok(result) => {
                            self.node_graph_view.load_from_data(
                                *root_id,
                                &result.nodes,
                                &result.edges,
                                &self.settings.node_graph,
                            );
                        }
                        Err(e) => tracing::error!("Failed to get trail: {}", e),
                    }
                }
            }
            Event::TrailConfigChange {
                depth,
                direction,
                edge_filter,
            } => {
                // Persist the trail configuration so graph controls remain consistent across
                // subsequent activations and restarts.
                self.settings.node_graph.trail_depth = *depth;
                self.settings.node_graph.trail_direction = *direction;
                self.settings.node_graph.trail_edge_filter = edge_filter.clone();
                self.settings.save();

                if self.show_trail_view
                    && let Some(root_id) = self.tab_manager.active_tab().and_then(|t| t.active_node)
                {
                    self.trail_controls.config.root_id = Some(root_id);
                    self.trail_controls.config.depth = *depth;
                    self.trail_controls.config.direction = *direction;
                    self.trail_controls.config.edge_filter = edge_filter.clone();

                    if let Some(storage) = &self.storage
                        && let Some(config) = self.trail_controls.config.to_trail_config()
                    {
                        match storage.get_trail(&config) {
                            Ok(result) => {
                                self.node_graph_view.load_from_data(
                                    root_id,
                                    &result.nodes,
                                    &result.edges,
                                    &self.settings.node_graph,
                                );
                            }
                            Err(e) => tracing::error!("Failed to refresh trail: {}", e),
                        }
                    }
                }
            }
            Event::BookmarkRemove { id } => {
                if let Some(storage) = &self.storage {
                    let _ = storage.delete_bookmark(*id);
                }
            }
            Event::BookmarkNavigate { node_id } => {
                self.event_bus.publish(Event::ActivateNode {
                    id: *node_id,
                    origin: ActivationOrigin::Sidebar,
                });
            }
            Event::BookmarkCategoryCreate { name } => {
                if let Some(storage) = &self.storage {
                    let _ = storage.create_bookmark_category(name);
                }
            }
            Event::BookmarkCategoryDelete { id } => {
                if let Some(storage) = &self.storage {
                    let _ = storage.delete_bookmark_category(*id);
                }
            }
            Event::TrailModeExit => {
                self.show_trail_view = false;
                if let Some(id) = self.tab_manager.active_tab().and_then(|t| t.active_node) {
                    self.select_node(id);
                }
            }
            Event::ErrorPanelToggle => {
                self.dock_state.focus_tab(TabId::Errors);
            }
            Event::ErrorFilterFile { file_id } => {
                self.error_panel.filter_by_file(*file_id);
                if file_id.is_none() {
                    self.error_panel.clear_filters();
                }
            }
            Event::ErrorNavigate { file_id, line } => {
                if let Some(storage) = &self.storage
                    && let Ok(Some(file_node)) = storage.get_node(*file_id)
                {
                    let path = std::path::PathBuf::from(file_node.serialized_name);
                    self.event_bus.publish(Event::ScrollToLine {
                        file: path,
                        line: *line as usize,
                    });
                }
            }
            Event::GraphNodeMove { .. } => {
                // Handled within NodeGraphView
            }
            Event::GraphNodeExpand { id, expand } => {
                let mut state = self.settings.node_graph.view_state.get_collapse_state(*id);
                state.is_collapsed = !expand;
                self.settings
                    .node_graph
                    .view_state
                    .set_collapse_state(*id, state);
                self.settings.save();
            }
            Event::GraphSectionExpand {
                id,
                section_kind,
                expand,
            } => {
                let kind = match section_kind.as_str() {
                    "FUNCTIONS" => codestory_graph::uml_types::VisibilityKind::Functions,
                    "VARIABLES" => codestory_graph::uml_types::VisibilityKind::Variables,
                    "PUBLIC" => codestory_graph::uml_types::VisibilityKind::Public,
                    "PRIVATE" => codestory_graph::uml_types::VisibilityKind::Private,
                    "PROTECTED" => codestory_graph::uml_types::VisibilityKind::Protected,
                    "INTERNAL" => codestory_graph::uml_types::VisibilityKind::Internal,
                    "OTHER" => codestory_graph::uml_types::VisibilityKind::Other,
                    _ => return,
                };
                let mut state = self.settings.node_graph.view_state.get_collapse_state(*id);
                if *expand {
                    state.collapsed_sections.remove(&kind);
                } else {
                    state.collapsed_sections.insert(kind);
                }
                self.settings
                    .node_graph
                    .view_state
                    .set_collapse_state(*id, state);
                self.settings.save();
            }
            Event::GraphNodeHide { .. } => {
                // Handled via internal state within NodeGraphView
            }
            Event::ExpandAll => {
                // Handled via handle_event in NodeGraphView
            }
            Event::CollapseAll => {
                // Handled via handle_event in NodeGraphView
            }
            Event::IndexingStarted { .. } => {
                self.is_indexing = true;
                self.status_message = "Indexing started...".to_string();
            }
            Event::IndexingProgress { current, total } => {
                self.status_message = format!("Indexing file {} of {}", current, total);
            }
            Event::IndexingComplete { duration_ms } => {
                self.is_indexing = false;
                self.status_message = "Indexing finished.".to_string();

                tracing::info!("Indexing complete in {}ms, refreshing metrics", duration_ms);

                // Reload data
                if let Some(storage) = &self.storage {
                    if let Ok(count) = storage.get_node_count() {
                        self.node_count = count;
                    }
                    // Refresh metrics
                    let metrics =
                        crate::components::metrics_panel::CodebaseMetrics::compute_from_storage(
                            storage,
                        );
                    self.metrics_panel.set_metrics(metrics);

                    if let Ok(nodes) = storage.get_nodes() {
                        let mut search_nodes = Vec::new();
                        self.node_names.clear();
                        for node in nodes {
                            let display_name = node
                                .qualified_name
                                .clone()
                                .unwrap_or_else(|| node.serialized_name.clone());
                            search_nodes.push((node.id, display_name.clone()));
                            self.node_names.insert(node.id, display_name.clone());
                            // Also update error panel's file name cache for file nodes
                            if node.kind == codestory_core::NodeKind::FILE {
                                self.error_panel
                                    .set_file_name(node.id, node.serialized_name);
                            }
                        }

                        if let Some(engine) = &mut self.search_engine
                            && let Err(e) = engine.index_nodes(search_nodes)
                        {
                            tracing::error!("Error indexing nodes for search: {}", e);
                        }
                    }
                }
            }
            Event::IndexingFailed { error } => {
                self.is_indexing = false;
                self.status_message = error.clone();

                if let Some(storage) = &self.storage {
                    let error_info = codestory_core::ErrorInfo {
                        message: error.clone(),
                        file_id: None,
                        line: None,
                        column: None,
                        is_fatal: true,
                        index_step: codestory_core::IndexStep::Indexing,
                    };
                    let _ = storage.insert_error(&error_info);
                }
            }
            Event::ProjectLoad { path } => {
                self.open_project(path.clone());
            }
            Event::ProjectOpened { .. } => {
                // Already handled by open_project calling internal logic
            }
            Event::SearchComplete {
                result_count,
                query,
            } => {
                self.status_message = format!("Found {} results for '{}'", result_count, query);
            }
            Event::SearchFailed { error } => {
                self.status_message = format!("Search failed: {}", error);
            }
            Event::ZoomToFit | Event::ZoomIn | Event::ZoomOut | Event::ZoomReset => {
                // Handled via handle_event in NodeGraphView
            }
            Event::SetTrailDepth(depth) => {
                self.settings.node_graph.trail_depth = *depth;
                self.settings.save();
                if let Some(id) = self.tab_manager.active_tab().and_then(|t| t.active_node) {
                    self.select_node_graph_detail(id);
                }
            }
            Event::SetGroupByFile(enabled) => {
                self.settings.node_graph.group_by_file = *enabled;
                self.settings.save();
                if let Some(id) = self.tab_manager.active_tab().and_then(|t| t.active_node) {
                    self.select_node_graph_detail(id);
                }
            }
            Event::SetGroupByNamespace(enabled) => {
                self.settings.node_graph.group_by_namespace = *enabled;
                self.settings.save();
                if let Some(id) = self.tab_manager.active_tab().and_then(|t| t.active_node) {
                    self.select_node_graph_detail(id);
                }
            }
            Event::SetLayoutMethod(algorithm) => {
                self.settings.node_graph.layout_algorithm = *algorithm;
                self.settings.save();
                if let Some(id) = self.tab_manager.active_tab().and_then(|t| t.active_node) {
                    self.select_node_graph_detail(id);
                }
            }
            Event::SetLayoutDirection(direction) => {
                self.settings.node_graph.layout_direction = *direction;
                self.settings.save();
                if let Some(id) = self.tab_manager.active_tab().and_then(|t| t.active_node) {
                    self.select_node_graph_detail(id);
                }
            }
            Event::SetShowClasses(visible) => {
                self.settings.node_graph.show_classes = *visible;
                self.settings.save();
                if let Some(id) = self.tab_manager.active_tab().and_then(|t| t.active_node) {
                    self.select_node_graph_detail(id);
                }
            }
            Event::SetShowFunctions(visible) => {
                self.settings.node_graph.show_functions = *visible;
                self.settings.save();
                if let Some(id) = self.tab_manager.active_tab().and_then(|t| t.active_node) {
                    self.select_node_graph_detail(id);
                }
            }
            Event::SetShowVariables(visible) => {
                self.settings.node_graph.show_variables = *visible;
                self.settings.save();
                if let Some(id) = self.tab_manager.active_tab().and_then(|t| t.active_node) {
                    self.select_node_graph_detail(id);
                }
            }
            Event::SetShowMinimap(visible) => {
                self.settings.node_graph.show_minimap = *visible;
                self.settings.save();
                // No need to reload data for minimap toggle
            }
            Event::SetShowLegend(visible) => {
                self.settings.node_graph.show_legend = *visible;
                self.settings.save();
                // No need to reload data for legend toggle
            }
            Event::OpenCustomTrailDialog => {
                if let Some(id) = self.tab_manager.active_tab().and_then(|t| t.active_node) {
                    let name = self.node_names.get(&id).cloned();
                    self.custom_trail_dialog.open_with_root(id, name);
                } else {
                    self.custom_trail_dialog.open();
                }
            }
            Event::NavigateToNode(id) => {
                self.select_node(*id);
            }
            Event::MetricsCompute => {
                // Compute metrics from storage and update panel
                if let Some(storage) = &self.storage {
                    let metrics =
                        crate::components::metrics_panel::CodebaseMetrics::compute_from_storage(
                            storage,
                        );
                    self.metrics_panel.set_metrics(metrics);
                }
            }
            Event::MetricsReady {
                total_files,
                total_lines,
                total_symbols,
            } => {
                tracing::info!(
                    "Metrics ready: {} files, {} lines, {} symbols",
                    total_files,
                    total_lines,
                    total_symbols
                );
            }
            Event::NeighborhoodLoaded {
                center_id,
                nodes,
                edges,
            } => {
                self.node_graph_view.load_from_data(
                    *center_id,
                    nodes,
                    edges,
                    &self.settings.node_graph,
                );
            }
            _ => {}
        }
    }
}

impl CodeStoryApp {
    fn handle_notification_event(&mut self, event: &Event) {
        if !self.settings.notifications.enabled {
            return;
        }

        match event {
            Event::IndexingStarted { file_count } => {
                if self.settings.notifications.show_indexing_progress {
                    self.notification_manager
                        .info(format!("Starting indexing of {} files...", file_count));
                }
            }
            Event::IndexingComplete { duration_ms } => {
                if self.settings.notifications.show_indexing_progress {
                    self.notification_manager.success(format!(
                        "Indexing complete in {:.2}s",
                        *duration_ms as f64 / 1000.0
                    ));
                }
            }
            Event::IndexingFailed { error } => {
                if self.settings.notifications.show_indexing_progress {
                    self.notification_manager
                        .error(format!("Indexing failed: {}", error));
                }
            }
            Event::ProjectOpened { path } => {
                if self.settings.notifications.show_file_operations {
                    self.notification_manager
                        .success(format!("Opened project: {}", path));
                }
            }
            Event::ProjectSaveFailed { error } => {
                if self.settings.notifications.show_file_operations {
                    self.notification_manager
                        .error(format!("Failed to save project: {}", error));
                }
            }
            Event::SearchComplete {
                result_count,
                query,
            } => {
                if self.settings.notifications.show_search_results {
                    if *result_count == 0 {
                        self.notification_manager
                            .warning(format!("No results found for '{}'", query));
                    } else {
                        self.notification_manager
                            .info(format!("Found {} results for '{}'", result_count, query));
                    }
                }
            }
            Event::ShowInfo { message } => self.notification_manager.info(message),
            Event::ShowSuccess { message } => self.notification_manager.success(message),
            Event::ShowWarning { message } => self.notification_manager.warning(message),
            Event::ShowError { message } => self.notification_manager.error(message),

            // Handle node collapse/expand
            Event::GraphNodeExpand { id, expand } => {
                let collapse_state = self
                    .settings
                    .node_graph
                    .view_state
                    .collapse_states
                    .entry(*id)
                    .or_insert_with(codestory_graph::uml_types::CollapseState::new);
                collapse_state.is_collapsed = !expand;

                // Settings are auto-saved periodically via the main update loop
            }

            // Handle section collapse/expand
            Event::GraphSectionExpand {
                id,
                section_kind,
                expand,
            } => {
                use std::str::FromStr;

                let collapse_state = self
                    .settings
                    .node_graph
                    .view_state
                    .collapse_states
                    .entry(*id)
                    .or_insert_with(codestory_graph::uml_types::CollapseState::new);

                if let Ok(visibility) =
                    codestory_graph::uml_types::VisibilityKind::from_str(section_kind)
                {
                    if *expand {
                        collapse_state.collapsed_sections.remove(&visibility);
                    } else {
                        collapse_state.collapsed_sections.insert(visibility);
                    }

                    // Settings are auto-saved periodically via the main update loop
                }
            }

            _ => {}
        }
    }

    /// Process file dialog results
    fn handle_file_dialog_results(&mut self) {
        if !self.file_dialog.is_open()
            && let DialogResult::Selected(path) = self.file_dialog.take_result()
            && let Some(callback_id) = self.file_dialog.callback_id()
        {
            let cid = callback_id.to_string();
            self.handle_file_selection(&cid, path);
        }
    }

    /// Handle file selection based on callback ID
    fn handle_file_selection(&mut self, callback_id: &str, path: std::path::PathBuf) {
        match callback_id {
            "open_project" => {
                self.open_project(path);
            }
            "select_project_directory" => {
                // TODO: Update ProjectWizard to use this path
                // ProjectWizard in Phase 3 uses a different model
                tracing::debug!("Project directory selected: {:?}", path);
            }
            "select_compilation_database" => {
                // TODO: Update ProjectWizard to use this path
                tracing::debug!("Compilation database selected: {:?}", path);
            }
            _ => {
                tracing::warn!("Unknown file dialog callback: {}", callback_id);
            }
        }
    }
}
