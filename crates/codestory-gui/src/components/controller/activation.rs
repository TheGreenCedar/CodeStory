use crate::components::code_view::enhanced::EnhancedCodeView;
use crate::components::code_view::multi_file::MultiFileCodeView;
use crate::components::commands::{ActivateNodeCommand, AppState, CommandHistory};
use crate::components::detail_panel::DetailPanel;
use crate::components::node_graph::NodeGraphView;
use crate::components::reference_list::ReferenceList;
use crate::components::sidebar::Sidebar;
use crate::navigation::TabManager;
use crate::settings::AppSettings;
use codestory_core::NodeId;
use codestory_events::{ActivationOrigin, Event, EventBus};
use codestory_storage::Storage;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct ActivationController {}

impl ActivationController {
    pub fn new(_event_bus: EventBus) -> Self {
        Self {}
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle_event(
        &mut self,
        event: &Event,
        storage: &Option<Storage>,
        settings: &AppSettings,
        tab_manager: &mut TabManager,
        code_view: &mut EnhancedCodeView,
        snippet_view: &mut MultiFileCodeView,
        node_graph_view: &mut NodeGraphView,
        detail_panel: &mut DetailPanel,
        reference_list: &mut ReferenceList,
        sidebar: &mut Sidebar,
        command_history: &mut CommandHistory,
        node_names: &HashMap<NodeId, String>,
    ) {
        match event {
            Event::ActivateNode { id, origin } => {
                self.activate_node(
                    *id,
                    origin.clone(),
                    storage,
                    settings,
                    tab_manager,
                    code_view,
                    snippet_view,
                    node_graph_view,
                    detail_panel,
                    reference_list,
                    sidebar,
                    command_history,
                    node_names,
                );
            }
            Event::CodeVisibleLineChanged { file, line } => {
                self.handle_code_scroll(file, *line, storage, node_graph_view);
            }
            _ => {}
        }
    }

    fn handle_code_scroll(
        &mut self,
        file: &str,
        line: usize,
        storage: &Option<Storage>,
        node_graph_view: &mut NodeGraphView,
    ) {
        if let Some(storage) = storage {
            // Find if there is a definition at this line in this file
            if let Ok(nodes) = storage.get_nodes_for_file_line(file, line as u32)
                && let Some(node) = nodes.first()
            {
                // If we found a node that is visible in the graph, request a pan
                node_graph_view.pending_pan_to_node = Some(node.id);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn activate_node(
        &mut self,
        id: NodeId,
        origin: ActivationOrigin,
        storage: &Option<Storage>,
        settings: &AppSettings,
        tab_manager: &mut TabManager,
        code_view: &mut EnhancedCodeView,
        snippet_view: &mut MultiFileCodeView,
        node_graph_view: &mut NodeGraphView,
        detail_panel: &mut DetailPanel,
        reference_list: &mut ReferenceList,
        sidebar: &mut Sidebar,
        command_history: &mut CommandHistory,
        node_names: &HashMap<NodeId, String>,
    ) {
        tracing::info!(
            "ActivationController: Activating node {:?} from {:?}",
            id,
            origin
        );

        // 1. Update Tab State & Command History
        if let Some(tab) = tab_manager.active_tab_mut() {
            let mut state = AppState {
                active_node: &mut tab.active_node,
            };
            let cmd = Box::new(ActivateNodeCommand::new(id));
            let _ = command_history.execute(cmd, &mut state);

            // Add to navigation history
            tab.history.push(crate::navigation::NavigationEntry {
                node_ids: vec![id],
                timestamp: std::time::Instant::now(),
                origin: origin.clone(),
            });

            // Update tab title
            if let Some(name) = node_names.get(&id) {
                tab.title = name.clone();
            }
        }

        // 2. Load Neighborhood/Trail for Graph View & Detail Panel
        if let Some(storage) = storage {
            // Update Graph View
            let trail_config = codestory_core::TrailConfig {
                root_id: id,
                mode: codestory_core::TrailMode::Neighborhood,
                target_id: None,
                depth: settings.node_graph.trail_depth,
                direction: settings.node_graph.trail_direction,
                edge_filter: settings.node_graph.trail_edge_filter.clone(),
                node_filter: Vec::new(),
                max_nodes: 500,
            };

            match storage.get_trail(&trail_config) {
                Ok(result) => {
                    node_graph_view.load_from_data(
                        id,
                        &result.nodes,
                        &result.edges,
                        &settings.node_graph,
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to get trail for neighborhood: {}", e);
                    if let Ok((nodes, edges)) = storage.get_neighborhood(id) {
                        node_graph_view.load_from_data(id, &nodes, &edges, &settings.node_graph);
                    }
                }
            }

            // Update Detail Panel
            match storage.get_node(id) {
                Ok(Some(node)) => match storage.get_neighborhood(id) {
                    Ok((_, edges)) => {
                        detail_panel.set_data(node, edges);
                    }
                    Err(e) => {
                        tracing::error!("Failed to get edges for detail: {}", e);
                    }
                },
                Ok(None) => tracing::warn!("Selected node not found: {}", id.0),
                Err(e) => tracing::error!("Error fetching selected node: {}", e),
            }

            // 3. Update Code View & Reference List
            match storage.get_occurrences_for_node(id) {
                Ok(occs) => {
                    let active_occ = occs
                        .iter()
                        .find(|occ| matches!(occ.kind, codestory_core::OccurrenceKind::DEFINITION))
                        .cloned()
                        .or_else(|| occs.first().cloned());
                    let active_file = active_occ.as_ref().map(|occ| occ.location.file_node_id);

                    reference_list.set_data(occs.clone(), active_file);

                    if let Some(active_occ) = active_occ.as_ref() {
                        let file_id = active_occ.location.file_node_id;
                        let file_occs: Vec<_> = occs
                            .iter()
                            .filter(|o| o.location.file_node_id == file_id)
                            .cloned()
                            .collect();

                        if let Ok(Some(file_node)) = storage.get_node(file_id) {
                            let path_str = file_node.serialized_name;
                            if std::path::Path::new(&path_str).exists()
                                && let Ok(content) = std::fs::read_to_string(&path_str)
                            {
                                code_view.set_file(
                                    path_str,
                                    content,
                                    active_occ.location.start_line as usize,
                                );
                                code_view.active_locations = vec![active_occ.location.clone()];
                                code_view.occurrences = file_occs;
                                sidebar
                                    .set_selected_path(std::path::PathBuf::from(&code_view.path));
                            }
                        }
                    }

                    snippet_view.clear();
                    if !occs.is_empty() {
                        let mut file_paths: HashMap<NodeId, PathBuf> = HashMap::new();
                        for occ in &occs {
                            let file_id = occ.location.file_node_id;
                            if !file_paths.contains_key(&file_id) {
                                if let Ok(Some(file_node)) = storage.get_node(file_id) {
                                    let path = PathBuf::from(file_node.serialized_name.clone());
                                    if path.exists()
                                        && let Ok(content) = std::fs::read_to_string(&path)
                                    {
                                        snippet_view.add_file(path.clone(), content, Some(file_id));
                                        file_paths.insert(file_id, path);
                                    }
                                }
                            }

                            if let Some(path) = file_paths.get(&file_id) {
                                snippet_view.add_occurrence(path, occ.clone());
                            }
                        }

                        if let Some(active_occ) = active_occ.as_ref() {
                            snippet_view.set_focus(active_occ.location.clone());
                        }

                        if let Ok(errors) = storage.get_errors(None) {
                            let mut counts = HashMap::new();
                            for error in errors {
                                if let Some(file_id) = error.file_id {
                                    *counts.entry(file_id).or_insert(0) += 1;
                                }
                            }
                            snippet_view.set_error_counts(counts);
                        }

                        snippet_view.sort_files_by_references();
                    }
                }
                Err(e) => tracing::error!("Failed to get occurrences: {}", e),
            }
        }
    }
}
