//! Tab Viewer Implementation
//!
//! Implements `egui_dock::TabViewer` to connect the docking system
//! with CodeStory's panel components.

use crate::dock_state::TabId;
use eframe::egui;
use egui_dock::tab_viewer::{OnCloseResponse, TabViewer};

/// Tab viewer that renders CodeStory panels within dock tabs.
///
/// This struct holds references to all the components that can be
/// rendered as tabs, allowing the docking system to delegate
/// rendering to the appropriate component based on the tab type.
///
/// # Usage
///
/// Create a `CodeStoryTabViewer` before calling `DockArea::show`:
///
/// ```ignore
/// let mut tab_viewer = CodeStoryTabViewer {
///     // ... component references
/// };
/// DockArea::new(&mut dock_state)
///     .show(ctx, &mut tab_viewer);
/// ```
pub struct CodeStoryTabViewer<'a> {
    /// Application theme for styling
    pub theme: &'a crate::theme::Theme,

    /// Code viewer component
    pub code_view: &'a mut crate::components::code_view::enhanced::EnhancedCodeView,

    /// Detail panel component
    pub detail_panel: &'a mut crate::components::detail_panel::DetailPanel,

    /// Bookmark panel component
    pub bookmark_panel: &'a mut crate::components::bookmark_panel::BookmarkPanel,

    /// Overview component
    pub overview: &'a mut crate::components::overview::ProjectOverview,

    /// Trail view controls
    pub trail_controls: &'a mut crate::components::trail_view::TrailViewControls,

    /// Node name lookup for bookmark display
    pub node_names: &'a std::collections::HashMap<codestory_core::NodeId, String>,

    /// Storage reference for error panel
    pub storage: &'a Option<codestory_storage::Storage>,

    /// Error panel reference
    pub error_panel: &'a mut crate::components::error_panel::ErrorPanel,

    /// Sidebar (file browser) component
    pub sidebar: &'a mut crate::components::sidebar::Sidebar,

    /// Reference list component
    pub reference_list: &'a mut crate::components::reference_list::ReferenceList,

    /// Event bus for publishing navigation events
    pub event_bus: &'a codestory_events::EventBus,

    /// Metrics panel component (Phase 4)
    pub metrics_panel: &'a mut crate::components::metrics_panel::MetricsPanel,

    /// Node graph view component (Phase 6)
    pub node_graph_view: &'a mut crate::components::node_graph::NodeGraphView,

    /// Snippet view component (Phase 2)
    pub snippet_view: &'a mut crate::components::code_view::multi_file::MultiFileCodeView,

    /// Application settings
    pub settings: &'a crate::settings::AppSettings,
}

impl<'a> TabViewer for CodeStoryTabViewer<'a> {
    type Tab = TabId;

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        tab.display_title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            TabId::Code => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.group(|ui| {
                        ui.set_min_height(300.0);
                        ui.set_min_height(300.0);
                        self.code_view.show(ui, self.theme);
                    });
                    ui.separator();
                    ui.group(|ui| {
                        if let Some(occ) = self.reference_list.ui(ui, self.node_names) {
                            self.event_bus
                                .publish(codestory_events::Event::ActivateNode {
                                    id: codestory_core::NodeId(occ.element_id),
                                    origin: codestory_events::ActivationOrigin::Code,
                                });
                        }
                    });
                });
            }
            TabId::Graph => {
                if let Some(clicked_id) =
                    self.node_graph_view
                        .show(ui, &self.settings.node_graph, self.theme.flavor)
                {
                    self.event_bus
                        .publish(codestory_events::Event::ActivateNode {
                            id: clicked_id,
                            origin: codestory_events::ActivationOrigin::Graph,
                        });
                }
            }
            TabId::Details => {
                self.detail_panel.ui(ui, self.bookmark_panel);
            }
            TabId::Errors => {
                self.error_panel.ui(ui, self.storage);
            }
            TabId::Bookmarks => {
                self.bookmark_panel.ui(ui, self.node_names);
            }
            TabId::Overview => {
                self.overview.ui(ui);
            }
            TabId::TrailControls => {
                self.trail_controls.ui(ui);
            }
            TabId::Metrics => {
                self.metrics_panel.render(ui);
            }
            TabId::ProjectTree => {
                if let Some(storage) = self.storage {
                    self.sidebar.ui(ui, Some(storage));
                } else {
                    self.sidebar.ui(ui, None);
                }
            }
            TabId::Snippets => {
                self.snippet_view.ui(ui);
            }
        }
    }

    fn closeable(&mut self, _tab: &mut Self::Tab) -> bool {
        // All tabs can be closed
        true
    }

    fn on_close(&mut self, _tab: &mut Self::Tab) -> OnCloseResponse {
        // Allow the tab to be closed
        OnCloseResponse::Close
    }

    fn context_menu(
        &mut self,
        ui: &mut egui::Ui,
        tab: &mut Self::Tab,
        _surface: egui_dock::SurfaceIndex,
        _node: egui_dock::NodeIndex,
    ) {
        ui.label(format!("Tab: {}", tab.title()));
        ui.separator();

        if ui.button("Close").clicked() {
            ui.close();
        }
    }
}
