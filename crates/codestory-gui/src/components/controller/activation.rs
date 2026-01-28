use crate::components::code_view::enhanced::EnhancedCodeView;
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
                depth: settings.node_graph.max_depth as u32,
                direction: codestory_core::TrailDirection::Both,
                edge_filter: Vec::new(),
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
                    reference_list.set_data(occs.clone());

                    if let Some(occ) = occs.first() {
                        let file_id = occ.location.file_node_id;
                        let file_locs: Vec<_> = occs
                            .iter()
                            .filter(|o| o.location.file_node_id == file_id)
                            .map(|o| o.location.clone())
                            .collect();

                        if let Ok(Some(file_node)) = storage.get_node(file_id) {
                            let path_str = file_node.serialized_name;
                            if std::path::Path::new(&path_str).exists()
                                && let Ok(content) = std::fs::read_to_string(&path_str)
                            {
                                code_view.set_file(
                                    path_str,
                                    content,
                                    occ.location.start_line as usize,
                                );
                                code_view.active_locations = file_locs;
                                sidebar
                                    .set_selected_path(std::path::PathBuf::from(&code_view.path));
                            }
                        }
                    }
                }
                Err(e) => tracing::error!("Failed to get occurrences: {}", e),
            }
        }
    }
}
