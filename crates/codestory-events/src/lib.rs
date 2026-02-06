use codestory_core::{EdgeId, EdgeKind, NodeId, SourceLocation};
use crossbeam_channel::{unbounded, Receiver, Sender};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ActivationOrigin {
    Graph,
    Code,
    Search,
    Sidebar,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RefreshMode {
    Incremental,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViewId {
    Graph,
    Code,
    Search,
    Sidebar,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TooltipInfo {
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LayoutAlgorithm {
    #[default]
    ForceDirected,
    Radial,
    Grid,
    Hierarchical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    // Activation
    ActivateNode {
        id: NodeId,
        origin: ActivationOrigin,
    },
    ActivateEdge {
        id: EdgeId,
    },
    ActivateTokens {
        ids: Vec<NodeId>,
    }, // Changed TokenId to NodeId for simplicity if they overlap
    DeactivateEdge {
        id: EdgeId,
    },

    // Graph
    GraphNodeExpand {
        id: NodeId,
        expand: bool,
    },
    GraphNodeMove {
        id: NodeId,
        x: f32,
        y: f32,
    },
    GraphNodeBundleSplit {
        id: NodeId,
    },
    GraphNodeHide {
        id: NodeId,
    },
    GraphSectionExpand {
        id: NodeId,
        section_kind: String,
        expand: bool,
    },

    // Navigation
    HistoryBack,
    HistoryForward,
    TabOpen {
        token_id: Option<NodeId>,
    },
    TabClose {
        index: usize,
    },
    TabSelect {
        index: usize,
    },

    // Search
    SearchQuery {
        query: String,
    },
    SearchAutocomplete {
        query: String,
    },

    // Indexing
    IndexingStarted {
        file_count: usize,
    },
    IndexingProgress {
        current: usize,
        total: usize,
    },
    IndexingFailed {
        error: String,
    },
    IndexingComplete {
        duration_ms: u64,
    },

    // Project
    ProjectOpened {
        path: String,
    },
    ProjectSaved {
        path: String,
    },
    ProjectSaveFailed {
        error: String,
    },
    ProjectLoad {
        path: PathBuf,
    },
    ProjectClose,
    ProjectRefresh {
        mode: RefreshMode,
    },

    // Search Results
    SearchComplete {
        result_count: usize,
        query: String,
    },
    SearchFailed {
        error: String,
    },

    // Notifications
    ShowInfo {
        message: String,
    },
    ShowSuccess {
        message: String,
    },
    ShowWarning {
        message: String,
    },
    ShowError {
        message: String,
    },

    // Code View
    ShowReference {
        location: SourceLocation,
    },
    ScrollToLine {
        file: PathBuf,
        line: usize,
    },
    CodeVisibleLineChanged {
        file: String,
        line: usize,
    },

    // UI
    FocusView {
        view: ViewId,
    },
    TooltipShow {
        info: TooltipInfo,
        x: f32,
        y: f32,
    },
    TooltipHide,
    StatusUpdate {
        message: String,
    },

    // ========================================================================
    // Bookmark Events
    // ========================================================================
    BookmarkAdd {
        node_id: NodeId,
        category_id: i64,
    },
    /// Add a bookmark to the default category, creating it if needed.
    BookmarkAddDefault {
        node_id: NodeId,
    },
    BookmarkRemove {
        id: i64,
    },
    BookmarkNavigate {
        node_id: NodeId,
    },
    BookmarkCategoryCreate {
        name: String,
    },
    BookmarkCategoryDelete {
        id: i64,
    },

    // ========================================================================
    // File Watcher Events
    // ========================================================================
    FilesChanged {
        paths: Vec<PathBuf>,
    },
    FileWatcherError {
        message: String,
    },
    FileWatcherEnabled {
        enabled: bool,
    },

    // ========================================================================
    // Trail View Events
    // ========================================================================
    TrailModeEnter {
        root_id: NodeId,
    },
    TrailModeExit,
    TrailConfigChange {
        depth: u32,
        direction: codestory_core::TrailDirection,
        edge_filter: Vec<EdgeKind>,
    },

    // ========================================================================
    // Undo/Redo Events
    // ========================================================================
    Undo,
    Redo,
    UndoStackChanged {
        can_undo: bool,
        can_redo: bool,
        undo_description: Option<String>,
        redo_description: Option<String>,
    },

    // ========================================================================
    // Error Panel Events
    // ========================================================================
    ErrorPanelToggle,
    ErrorNavigate {
        file_id: NodeId,
        line: u32,
    },
    ErrorFilterChange {
        fatal_only: bool,
        indexed_only: bool,
    },
    /// Filter errors by file ID
    ErrorFilterFile {
        file_id: Option<NodeId>,
    },

    // ========================================================================
    // Graph Controls Events (Phase 1)
    // ========================================================================
    /// Navigate to a node (for history navigation)
    NavigateToNode(NodeId),
    /// Expand all nodes in the graph
    ExpandAll,
    /// Collapse all nodes in the graph
    CollapseAll,
    /// Set the trail depth (`0` means "infinite", bounded by node caps).
    SetTrailDepth(u32),
    /// Set group by file mode
    SetGroupByFile(bool),
    /// Set group by namespace mode
    SetGroupByNamespace(bool),
    /// Open the custom trail dialog
    OpenCustomTrailDialog,
    /// Zoom to fit the entire graph
    ZoomToFit,
    /// Zoom in the graph viewport
    ZoomIn,
    /// Zoom out the graph viewport
    ZoomOut,
    /// Reset the graph viewport zoom to 100% (1.0)
    ZoomReset,
    /// Set the graph layout method
    SetLayoutMethod(LayoutAlgorithm),
    /// Set the graph layout direction (Horizontal or Vertical)
    SetLayoutDirection(codestory_core::LayoutDirection),
    /// Set visibility of classes in the graph
    SetShowClasses(bool),
    /// Set visibility of functions in the graph
    SetShowFunctions(bool),
    /// Set visibility of variables in the graph
    SetShowVariables(bool),
    /// Set visibility of the potential minimap
    SetShowMinimap(bool),
    /// Set visibility of the potential legend
    SetShowLegend(bool),

    // ========================================================================
    // Metrics Events (Phase 4)
    // ========================================================================
    /// Request metrics computation
    MetricsCompute,
    /// Metrics computation completed
    /// Metrics computation completed
    MetricsReady {
        total_files: usize,
        total_lines: usize,
        total_symbols: usize,
    },

    // ========================================================================
    // Node Graph Events (Phase 6)
    // ========================================================================
    NeighborhoodLoaded {
        center_id: NodeId,
        nodes: Vec<codestory_core::Node>,
        edges: Vec<codestory_core::Edge>,
    },
}

#[derive(Clone)]
pub struct EventBus {
    tx: Sender<Event>,
    rx: Receiver<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx }
    }

    pub fn sender(&self) -> Sender<Event> {
        self.tx.clone()
    }

    pub fn receiver(&self) -> Receiver<Event> {
        self.rx.clone()
    }

    pub fn publish(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    /// Dispatch all pending events to a listener.
    /// This is useful for processing events in the UI loop.
    pub fn dispatch_to<L: EventListener>(&self, listener: &mut L) {
        while let Ok(event) = self.rx.try_recv() {
            listener.handle_event(&event);
        }
    }
}

/// Trait for components that respond to events.
/// Implement this to receive events from the EventBus.
pub trait EventListener {
    fn handle_event(&mut self, event: &Event);
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_core::NodeId;

    #[test]
    fn test_event_bus_publish_receive() {
        let bus = EventBus::new();
        let sender = bus.sender();
        let receiver = bus.receiver();

        let event = Event::ActivateNode {
            id: NodeId(123),
            origin: ActivationOrigin::Graph,
        };

        sender.send(event.clone()).unwrap();

        let received = receiver.recv().unwrap();
        match received {
            Event::ActivateNode { id, origin } => {
                assert_eq!(id.0, 123);
                assert!(matches!(origin, ActivationOrigin::Graph));
            }
            _ => panic!("Expected ActivateNode event"),
        }
    }

    #[test]
    fn test_indexing_events() {
        let bus = EventBus::new();
        bus.publish(Event::IndexingStarted { file_count: 10 });
        bus.publish(Event::IndexingProgress {
            current: 5,
            total: 10,
        });
        bus.publish(Event::IndexingComplete { duration_ms: 100 });

        let rx = bus.receiver();
        if let Event::IndexingStarted { file_count } = rx.recv().unwrap() {
            assert_eq!(file_count, 10);
        } else {
            panic!("Expected IndexingStarted");
        }

        if let Event::IndexingProgress { current, total } = rx.recv().unwrap() {
            assert_eq!(current, 5);
            assert_eq!(total, 10);
        } else {
            panic!("Expected IndexingProgress");
        }

        if let Event::IndexingComplete { duration_ms } = rx.recv().unwrap() {
            assert_eq!(duration_ms, 100);
        } else {
            panic!("Expected IndexingComplete");
        }
    }
}
