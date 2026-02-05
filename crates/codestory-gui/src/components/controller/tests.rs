use super::activation::ActivationController;
use crate::components::code_view::enhanced::EnhancedCodeView;
use crate::components::code_view::multi_file::MultiFileCodeView;
use crate::components::commands::CommandHistory;
use crate::components::detail_panel::DetailPanel;
use crate::components::node_graph::NodeGraphView;
use crate::components::reference_list::ReferenceList;
use crate::components::sidebar::Sidebar;
use crate::navigation::TabManager;
use crate::settings::AppSettings;
use codestory_core::NodeId;
use codestory_events::{ActivationOrigin, Event, EventBus};
use std::collections::HashMap;

#[test]
fn test_activation_updates_tab_title() {
    let event_bus = EventBus::new();
    let mut controller = ActivationController::new(event_bus);

    let mut tab_manager = TabManager::new();
    tab_manager.open_tab("Test".to_string(), None);

    let mut code_view = EnhancedCodeView::new();
    let mut snippet_view = MultiFileCodeView::new();
    let mut node_graph_view = NodeGraphView::new(EventBus::new());
    let mut detail_panel = DetailPanel::new();
    let mut reference_list = ReferenceList::new();
    let mut sidebar = Sidebar::new();
    let mut command_history = CommandHistory::new(10, EventBus::new());
    let mut node_names = HashMap::new();

    let node_id = NodeId(1);
    node_names.insert(node_id, "TestNode".to_string());

    let event = Event::ActivateNode {
        id: node_id,
        origin: ActivationOrigin::Graph,
    };

    controller.handle_event(
        &event,
        &None, // storage
        &AppSettings::default(),
        &mut tab_manager,
        &mut code_view,
        &mut snippet_view,
        &mut node_graph_view,
        &mut detail_panel,
        &mut reference_list,
        &mut sidebar,
        &mut command_history,
        &node_names,
    );

    let active_tab = tab_manager.active_tab().unwrap();
    assert_eq!(active_tab.title, "TestNode");
    assert_eq!(active_tab.active_node, Some(node_id));
}
