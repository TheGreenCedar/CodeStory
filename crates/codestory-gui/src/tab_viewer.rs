//! Tab Viewer Implementation
//!
//! Implements `egui_dock::TabViewer` to connect the docking system
//! with CodeStory's panel components.

use crate::components::code_view::{ClickAction, CodeViewMode};
use crate::dock_state::TabId;
use eframe::egui;
use egui_dock::tab_viewer::{OnCloseResponse, TabViewer};
use egui_phosphor::regular as ph;

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

    /// Code view mode
    pub code_view_mode: &'a mut CodeViewMode,

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
    pub settings: &'a mut crate::settings::AppSettings,
    pub settings_dirty: &'a mut bool,
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
                    ui.horizontal(|ui| {
                        ui.selectable_value(self.code_view_mode, CodeViewMode::SingleFile, "File");
                        ui.selectable_value(
                            self.code_view_mode,
                            CodeViewMode::Snippets,
                            "Snippets",
                        );
                        ui.separator();
                        if ui
                            .button(ph::WARNING_CIRCLE)
                            .on_hover_text("Show Errors")
                            .clicked()
                        {
                            self.event_bus
                                .publish(codestory_events::Event::ErrorPanelToggle);
                        }
                        ui.separator();
                        let ref_label = self.reference_list.position_label();
                        let refs = self.reference_list.filtered_len();
                        let prev_clicked = ui
                            .button(ph::CARET_UP)
                            .on_hover_text("Previous Reference")
                            .clicked();
                        let next_clicked = ui
                            .button(ph::CARET_DOWN)
                            .on_hover_text("Next Reference")
                            .clicked();
                        ui.label(format!("Refs {}", ref_label));
                        if refs == 0 {
                            ui.add_space(6.0);
                        }

                        if prev_clicked {
                            if let Some(occ) = self.reference_list.prev_occurrence() {
                                self.event_bus
                                    .publish(codestory_events::Event::ShowReference {
                                        location: occ.location,
                                    });
                            }
                        }
                        if next_clicked {
                            if let Some(occ) = self.reference_list.next_occurrence() {
                                self.event_bus
                                    .publish(codestory_events::Event::ShowReference {
                                        location: occ.location,
                                    });
                            }
                        }
                    });
                    ui.separator();

                    match *self.code_view_mode {
                        CodeViewMode::SingleFile => {
                            ui.group(|ui| {
                                ui.set_min_height(300.0);
                                ui.set_min_height(300.0);
                                self.code_view.show(ui, self.theme);
                            });
                            ui.separator();
                            ui.group(|ui| {
                                if let Some(occ) = self.reference_list.ui(ui, self.node_names) {
                                    self.event_bus.publish(
                                        codestory_events::Event::ShowReference {
                                            location: occ.location,
                                        },
                                    );
                                }
                            });
                        }
                        CodeViewMode::Snippets => {
                            if let Some(action) = self.snippet_view.ui(ui) {
                                match action {
                                    ClickAction::NavigateToLocation(location) => {
                                        self.event_bus.publish(
                                            codestory_events::Event::ShowReference { location },
                                        );
                                    }
                                    ClickAction::NavigateToLine(path, line) => {
                                        self.event_bus.publish(
                                            codestory_events::Event::ScrollToLine {
                                                file: path,
                                                line,
                                            },
                                        );
                                    }
                                    ClickAction::OpenFile(path) => {
                                        self.event_bus.publish(
                                            codestory_events::Event::ScrollToLine {
                                                file: path,
                                                line: 1,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }
                });
            }
            TabId::Graph => {
                let response = self.node_graph_view.show(
                    ui,
                    &mut self.settings.node_graph,
                    self.theme.mode,
                    self.settings.animation_speed,
                );
                if let Some(clicked_id) = response.clicked_node {
                    self.event_bus
                        .publish(codestory_events::Event::ActivateNode {
                            id: clicked_id,
                            origin: codestory_events::ActivationOrigin::Graph,
                        });
                }
                if response.view_state_dirty {
                    *self.settings_dirty = true;
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
                ui.horizontal(|ui| {
                    let ref_label = self.reference_list.position_label();
                    let prev_clicked = ui
                        .button(ph::CARET_UP)
                        .on_hover_text("Previous Reference")
                        .clicked();
                    let next_clicked = ui
                        .button(ph::CARET_DOWN)
                        .on_hover_text("Next Reference")
                        .clicked();
                    ui.label(format!("Refs {}", ref_label));

                    if prev_clicked {
                        if let Some(occ) = self.reference_list.prev_occurrence() {
                            self.event_bus
                                .publish(codestory_events::Event::ShowReference {
                                    location: occ.location,
                                });
                        }
                    }
                    if next_clicked {
                        if let Some(occ) = self.reference_list.next_occurrence() {
                            self.event_bus
                                .publish(codestory_events::Event::ShowReference {
                                    location: occ.location,
                                });
                        }
                    }
                });
                ui.separator();
                if let Some(action) = self.snippet_view.ui(ui) {
                    match action {
                        ClickAction::NavigateToLocation(location) => {
                            self.event_bus
                                .publish(codestory_events::Event::ShowReference { location });
                        }
                        ClickAction::NavigateToLine(path, line) => {
                            self.event_bus
                                .publish(codestory_events::Event::ScrollToLine {
                                    file: path,
                                    line,
                                });
                        }
                        ClickAction::OpenFile(path) => {
                            self.event_bus
                                .publish(codestory_events::Event::ScrollToLine {
                                    file: path,
                                    line: 1,
                                });
                        }
                    }
                }
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
