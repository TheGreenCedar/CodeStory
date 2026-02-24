use codestory_api::{
    ApiError, AppEventPayload, EdgeId, EdgeKind, GraphEdgeDto, GraphNodeDto, GraphRequest,
    GraphResponse, IndexMode, NodeDetailsDto, NodeDetailsRequest, NodeId, NodeKind,
    OpenProjectRequest, ProjectSummary, ReadFileTextRequest, ReadFileTextResponse, SearchHit,
    SearchRequest, SetUiLayoutRequest, StartIndexingRequest, StorageStatsDto, TrailConfigDto,
};
use codestory_events::{Event, EventBus};
use codestory_search::SearchEngine;
use codestory_storage::Storage;
use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn no_project_error() -> ApiError {
    ApiError::invalid_argument("No project open. Call open_project first.")
}

fn node_display_name(node: &codestory_core::Node) -> String {
    node.qualified_name
        .clone()
        .unwrap_or_else(|| node.serialized_name.clone())
}

fn clamp_i64_to_u32(v: i64) -> u32 {
    if v <= 0 {
        0
    } else if v > u32::MAX as i64 {
        u32::MAX
    } else {
        v as u32
    }
}

fn build_search_state(
    nodes: Vec<codestory_core::Node>,
) -> Result<(HashMap<codestory_core::NodeId, String>, SearchEngine), ApiError> {
    let mut node_names = HashMap::new();
    let mut search_nodes = Vec::with_capacity(nodes.len());
    for node in nodes {
        let display_name = node_display_name(&node);
        node_names.insert(node.id, display_name.clone());
        search_nodes.push((node.id, display_name));
    }

    let mut engine = SearchEngine::new(None)
        .map_err(|e| ApiError::internal(format!("Failed to init search engine: {e}")))?;
    engine
        .index_nodes(search_nodes)
        .map_err(|e| ApiError::internal(format!("Failed to index search nodes: {e}")))?;

    Ok((node_names, engine))
}

struct AppState {
    project_root: Option<PathBuf>,
    storage_path: Option<PathBuf>,
    node_names: HashMap<codestory_core::NodeId, String>,
    search_engine: Option<SearchEngine>,
    is_indexing: bool,
}

/// GUI-agnostic orchestrator for CodeStory.
///
/// This is intentionally "headless": any app shell (CLI, desktop, IDE integration)
/// should call methods on this controller and subscribe to `AppEventPayload`.
#[derive(Clone)]
pub struct AppController {
    state: Arc<Mutex<AppState>>,
    events_tx: Sender<AppEventPayload>,
    events_rx: Receiver<AppEventPayload>,
}

impl Default for AppController {
    fn default() -> Self {
        Self::new()
    }
}

impl AppController {
    pub fn new() -> Self {
        let (events_tx, events_rx) = unbounded();
        Self {
            state: Arc::new(Mutex::new(AppState {
                project_root: None,
                storage_path: None,
                node_names: HashMap::new(),
                search_engine: None,
                is_indexing: false,
            })),
            events_tx,
            events_rx,
        }
    }

    /// Subscribe to backend events. Intended to be consumed by a single pump
    /// that forwards to the active runtime.
    pub fn events(&self) -> Receiver<AppEventPayload> {
        self.events_rx.clone()
    }

    fn require_project_root(&self) -> Result<PathBuf, ApiError> {
        self.state
            .lock()
            .project_root
            .clone()
            .ok_or_else(no_project_error)
    }

    fn require_storage_path(&self) -> Result<PathBuf, ApiError> {
        self.state
            .lock()
            .storage_path
            .clone()
            .ok_or_else(no_project_error)
    }

    pub fn open_project(&self, req: OpenProjectRequest) -> Result<ProjectSummary, ApiError> {
        let root = PathBuf::from(req.path);
        if !root.exists() {
            return Err(ApiError::not_found(format!(
                "Project path does not exist: {}",
                root.display()
            )));
        }
        if !root.is_dir() {
            return Err(ApiError::invalid_argument(format!(
                "Project path is not a directory: {}",
                root.display()
            )));
        }

        let storage_path = root.join("codestory.db");
        let storage = Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;

        let stats = storage
            .get_stats()
            .map_err(|e| ApiError::internal(format!("Failed to query stats: {e}")))?;

        // Build symbol cache + search index.
        let nodes = storage
            .get_nodes()
            .map_err(|e| ApiError::internal(format!("Failed to load nodes: {e}")))?;
        let (node_names, engine) = build_search_state(nodes)?;

        {
            let mut s = self.state.lock();
            s.project_root = Some(root.clone());
            s.storage_path = Some(storage_path);
            s.node_names = node_names;
            s.search_engine = Some(engine);
        }

        let dto_stats = StorageStatsDto {
            node_count: clamp_i64_to_u32(stats.node_count),
            edge_count: clamp_i64_to_u32(stats.edge_count),
            file_count: clamp_i64_to_u32(stats.file_count),
            error_count: clamp_i64_to_u32(stats.error_count),
        };

        let _ = self.events_tx.send(AppEventPayload::StatusUpdate {
            message: "Project opened.".to_string(),
        });

        Ok(ProjectSummary {
            root: root.to_string_lossy().to_string(),
            stats: dto_stats,
        })
    }

    pub fn start_indexing(&self, req: StartIndexingRequest) -> Result<(), ApiError> {
        let (root, storage_path) = {
            let mut s = self.state.lock();
            if s.is_indexing {
                return Ok(());
            }
            let root = s.project_root.clone().ok_or_else(|| {
                ApiError::invalid_argument("No project open. Call open_project first.")
            })?;
            let storage_path = s
                .storage_path
                .clone()
                .unwrap_or_else(|| root.join("codestory.db"));
            s.is_indexing = true;
            (root, storage_path)
        };

        let events_tx = self.events_tx.clone();
        let controller = self.clone();

        // Use a dedicated thread so callers can keep their runtime responsive.
        std::thread::spawn(move || {
            let result = match req.mode {
                IndexMode::Full => index_full(&root, &storage_path, &events_tx),
                IndexMode::Incremental => index_incremental(&root, &storage_path, &events_tx),
            };

            if let Err(err) = result {
                let _ = events_tx.send(AppEventPayload::IndexingFailed { error: err.message });
            }

            // Mark indexing as complete and refresh caches if possible.
            if let Ok(storage) = Storage::open(&storage_path) {
                refresh_caches(&controller, &storage);
            } else {
                controller.state.lock().is_indexing = false;
            }
        });

        Ok(())
    }

    pub fn search(&self, req: SearchRequest) -> Result<Vec<SearchHit>, ApiError> {
        let (storage_path, matches, node_names) = {
            let mut s = self.state.lock();
            let engine = s.search_engine.as_mut().ok_or_else(|| {
                ApiError::invalid_argument("Search engine not initialized. Open a project first.")
            })?;
            let matches = engine.search_symbol_with_scores(&req.query);
            let storage_path = s.storage_path.clone().ok_or_else(no_project_error)?;
            let node_names = s.node_names.clone();
            (storage_path, matches, node_names)
        };

        let storage = Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;

        let mut hits = Vec::new();
        for (id, score) in matches {
            let display_name = node_names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| id.0.to_string());

            let mut file_path = None;
            let mut line = None;
            if let Ok(occs) = storage.get_occurrences_for_node(id)
                && let Some(occ) = occs.first()
            {
                if let Ok(Some(file_node)) = storage.get_node(occ.location.file_node_id) {
                    file_path = Some(file_node.serialized_name);
                }
                line = Some(occ.location.start_line);
            }

            let kind = match storage.get_node(id) {
                Ok(Some(node)) => NodeKind::from(node.kind),
                _ => NodeKind::UNKNOWN,
            };

            hits.push(SearchHit {
                node_id: NodeId::from(id),
                display_name,
                kind,
                file_path,
                line,
                score,
            });
        }

        Ok(hits)
    }

    pub fn graph_neighborhood(&self, req: GraphRequest) -> Result<GraphResponse, ApiError> {
        let storage_path = self.require_storage_path()?;

        let center = req.center_id.to_core()?;

        let storage = Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;

        let max_edges = req.max_edges.unwrap_or(400).min(2_000) as usize;
        let mut edges = storage
            .get_edges_for_node_id(center)
            .map_err(|e| ApiError::internal(format!("Failed to load edges: {e}")))?;
        edges.sort_by_key(|e| e.id.0);
        let mut truncated = false;
        if edges.len() > max_edges {
            edges.truncate(max_edges);
            truncated = true;
        }

        let mut ordered_node_ids = Vec::new();
        let mut seen = HashSet::new();
        ordered_node_ids.push(center);
        seen.insert(center);

        let mut edge_dtos = Vec::with_capacity(edges.len());
        for edge in edges {
            let edge = edge.with_effective_endpoints();
            let (source, target) = (edge.source, edge.target);

            edge_dtos.push(GraphEdgeDto {
                id: EdgeId::from(edge.id),
                source: NodeId::from(source),
                target: NodeId::from(target),
                kind: EdgeKind::from(edge.kind),
            });

            if seen.insert(source) {
                ordered_node_ids.push(source);
            }
            if seen.insert(target) {
                ordered_node_ids.push(target);
            }
        }

        // Pull labels from cached names for a small subset of nodes.
        let labels_by_id = {
            let s = self.state.lock();
            let mut labels = HashMap::new();
            for id in &ordered_node_ids {
                if let Some(name) = s.node_names.get(id) {
                    labels.insert(*id, name.clone());
                }
            }
            labels
        };

        let mut node_dtos = Vec::with_capacity(ordered_node_ids.len());
        for id in ordered_node_ids {
            let label = labels_by_id
                .get(&id)
                .cloned()
                .unwrap_or_else(|| id.0.to_string());
            let kind = match storage.get_node(id) {
                Ok(Some(node)) => NodeKind::from(node.kind),
                _ => NodeKind::UNKNOWN,
            };

            node_dtos.push(GraphNodeDto {
                id: NodeId::from(id),
                label,
                kind,
                depth: if id == center { 0 } else { 1 },
            });
        }

        Ok(GraphResponse {
            center_id: NodeId::from(center),
            nodes: node_dtos,
            edges: edge_dtos,
            truncated,
        })
    }

    pub fn graph_trail(&self, req: TrailConfigDto) -> Result<GraphResponse, ApiError> {
        let storage_path = self.require_storage_path()?;

        let root_id = req.root_id.to_core()?;
        let target_id = match req.target_id {
            Some(id) => Some(id.to_core()?),
            None => None,
        };

        let config = codestory_core::TrailConfig {
            root_id,
            mode: req.mode.into(),
            target_id,
            depth: req.depth,
            direction: req.direction.into(),
            edge_filter: req.edge_filter.into_iter().map(Into::into).collect(),
            node_filter: req.node_filter.into_iter().map(Into::into).collect(),
            max_nodes: req.max_nodes.clamp(10, 100_000) as usize,
        };

        let storage = Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;

        let result = storage
            .get_trail(&config)
            .map_err(|e| ApiError::internal(format!("Failed to compute trail: {e}")))?;

        let codestory_core::TrailResult {
            nodes,
            edges,
            depth_map,
            truncated,
        } = result;

        // Pull labels from cached names for the returned node set.
        let labels_by_id = {
            let s = self.state.lock();
            let mut labels = HashMap::new();
            for node in &nodes {
                if let Some(name) = s.node_names.get(&node.id) {
                    labels.insert(node.id, name.clone());
                }
            }
            labels
        };

        let mut node_dtos = Vec::with_capacity(nodes.len());
        for node in nodes {
            let label = labels_by_id.get(&node.id).cloned().unwrap_or_else(|| {
                node.qualified_name
                    .clone()
                    .unwrap_or_else(|| node.serialized_name.clone())
            });
            let depth = depth_map.get(&node.id).copied().unwrap_or(0);

            node_dtos.push(GraphNodeDto {
                id: NodeId::from(node.id),
                label,
                kind: NodeKind::from(node.kind),
                depth,
            });
        }

        let mut edge_dtos = Vec::with_capacity(edges.len());
        for edge in edges {
            let edge = edge.with_effective_endpoints();
            edge_dtos.push(GraphEdgeDto {
                id: EdgeId::from(edge.id),
                source: NodeId::from(edge.source),
                target: NodeId::from(edge.target),
                kind: EdgeKind::from(edge.kind),
            });
        }

        Ok(GraphResponse {
            center_id: NodeId::from(config.root_id),
            nodes: node_dtos,
            edges: edge_dtos,
            truncated,
        })
    }

    pub fn node_details(&self, req: NodeDetailsRequest) -> Result<NodeDetailsDto, ApiError> {
        let storage_path = self.require_storage_path()?;

        let id = req.id.to_core()?;

        let storage = Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;

        let node = storage
            .get_node(id)
            .map_err(|e| ApiError::internal(format!("Failed to query node: {e}")))?
            .ok_or_else(|| ApiError::not_found(format!("Node not found: {id}")))?;

        let display_name = self
            .state
            .lock()
            .node_names
            .get(&node.id)
            .cloned()
            .unwrap_or_else(|| {
                node.qualified_name
                    .clone()
                    .unwrap_or_else(|| node.serialized_name.clone())
            });

        let file_path = match node.file_node_id {
            Some(file_id) => match storage.get_node(file_id) {
                Ok(Some(file_node)) => Some(file_node.serialized_name),
                _ => None,
            },
            None => None,
        };

        Ok(NodeDetailsDto {
            id: NodeId::from(node.id),
            kind: NodeKind::from(node.kind),
            display_name,
            serialized_name: node.serialized_name,
            qualified_name: node.qualified_name,
            canonical_id: node.canonical_id,
            file_path,
            start_line: node.start_line,
            start_col: node.start_col,
            end_line: node.end_line,
            end_col: node.end_col,
        })
    }

    pub fn read_file_text(
        &self,
        req: ReadFileTextRequest,
    ) -> Result<ReadFileTextResponse, ApiError> {
        let root = self.require_project_root()?;

        let root = root
            .canonicalize()
            .map_err(|e| ApiError::internal(format!("Failed to resolve project root: {e}")))?;

        let raw = PathBuf::from(req.path);
        let candidate = if raw.is_absolute() {
            raw
        } else {
            root.join(raw)
        };

        let candidate = candidate.canonicalize().map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                ApiError::not_found(format!("File not found: {}", candidate.display()))
            } else {
                ApiError::internal(format!("Failed to resolve file path: {e}"))
            }
        })?;

        if !candidate.starts_with(&root) {
            return Err(ApiError::invalid_argument(
                "Refusing to read file outside project root.",
            ));
        }

        let text = std::fs::read_to_string(&candidate).map_err(|e| {
            ApiError::internal(format!("Failed to read file {}: {e}", candidate.display()))
        })?;

        Ok(ReadFileTextResponse {
            path: candidate.to_string_lossy().to_string(),
            text,
        })
    }

    pub fn get_ui_layout(&self) -> Result<Option<String>, ApiError> {
        let root = self.require_project_root()?;
        let path = root.join("codestory_ui.json");
        match std::fs::read_to_string(&path) {
            Ok(contents) => Ok(Some(contents)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ApiError::internal(format!(
                "Failed to read UI layout {}: {e}",
                path.display()
            ))),
        }
    }

    pub fn set_ui_layout(&self, req: SetUiLayoutRequest) -> Result<(), ApiError> {
        let root = self.require_project_root()?;
        let path = root.join("codestory_ui.json");
        std::fs::write(&path, req.json).map_err(|e| {
            ApiError::internal(format!("Failed to write UI layout {}: {e}", path.display()))
        })?;
        Ok(())
    }
}

fn index_full(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
) -> Result<(), ApiError> {
    run_indexing_common(root, storage_path, events_tx, true, |project, _storage| {
        project
            .full_refresh()
            .map_err(|e| ApiError::internal(format!("Failed to collect files: {e}")))
    })
}

fn index_incremental(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
) -> Result<(), ApiError> {
    run_indexing_common(root, storage_path, events_tx, false, |project, storage| {
        project
            .generate_refresh_info(storage)
            .map_err(|e| ApiError::internal(format!("Failed to generate refresh info: {e}")))
    })
}

fn spawn_progress_forwarder(
    rx: Receiver<Event>,
    progress_tx: Sender<AppEventPayload>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(ev) = rx.recv() {
            if let Event::IndexingProgress { current, total } = ev {
                let _ = progress_tx.send(AppEventPayload::IndexingProgress {
                    current: current.min(u32::MAX as usize) as u32,
                    total: total.min(u32::MAX as usize) as u32,
                });
            }
        }
    })
}

fn run_indexing_common<F>(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    clear_storage: bool,
    refresh_builder: F,
) -> Result<(), ApiError>
where
    F: FnOnce(&codestory_project::Project, &Storage) -> Result<codestory_project::RefreshInfo, ApiError>,
{
    let start_time = std::time::Instant::now();

    let mut storage = Storage::open(storage_path)
        .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))?;

    if clear_storage {
        storage
            .clear()
            .map_err(|e| ApiError::internal(format!("Failed to clear storage: {e}")))?;
    }

    let project = codestory_project::Project::open(root.to_path_buf())
        .map_err(|e| ApiError::internal(format!("Failed to open project: {e}")))?;

    let refresh_info = refresh_builder(&project, &storage)?;

    let total_files = refresh_info.files_to_index.len().min(u32::MAX as usize) as u32;
    let _ = events_tx.send(AppEventPayload::IndexingStarted {
        file_count: total_files,
    });

    let bus = EventBus::new();
    let forwarder = spawn_progress_forwarder(bus.receiver(), events_tx.clone());

    let indexer = codestory_index::WorkspaceIndexer::new(root.to_path_buf());
    let result = indexer.run_incremental(&mut storage, &refresh_info, &bus, None);

    // Drop bus so forwarder unblocks.
    drop(bus);
    let _ = forwarder.join();

    if let Err(e) = result {
        return Err(ApiError::internal(format!("Indexing failed: {e}")));
    }

    let _ = events_tx.send(AppEventPayload::IndexingComplete {
        duration_ms: start_time.elapsed().as_millis().min(u32::MAX as u128) as u32,
    });
    Ok(())
}

fn refresh_caches(controller: &AppController, storage: &Storage) {
    let refreshed = storage
        .get_nodes()
        .ok()
        .and_then(|nodes| build_search_state(nodes).ok());

    let mut s = controller.state.lock();
    if let Some((node_names, engine)) = refreshed {
        s.node_names = node_names;
        s.search_engine = Some(engine);
    }
    s.is_indexing = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_core::{Node, NodeId, NodeKind};
    use crossbeam_channel::unbounded;

    #[test]
    fn build_search_state_prefers_qualified_name() {
        let nodes = vec![Node {
            id: NodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "short_name".to_string(),
            qualified_name: Some("pkg.mod.short_name".to_string()),
            ..Default::default()
        }];

        let (node_names, mut engine) = build_search_state(nodes).expect("build search state");
        assert_eq!(
            node_names.get(&NodeId(1)).map(String::as_str),
            Some("pkg.mod.short_name")
        );

        let hits = engine.search_symbol("pkg.mod");
        assert_eq!(hits.first().copied(), Some(NodeId(1)));
    }

    #[test]
    fn progress_forwarder_relay_only_progress_events() {
        let (event_tx, event_rx) = unbounded::<Event>();
        let (app_tx, app_rx) = unbounded::<AppEventPayload>();
        let handle = spawn_progress_forwarder(event_rx, app_tx);

        event_tx
            .send(Event::IndexingProgress {
                current: 3,
                total: 5,
            })
            .expect("send progress event");
        event_tx
            .send(Event::StatusUpdate {
                message: "ignore me".to_string(),
            })
            .expect("send status event");
        drop(event_tx);

        let forwarded = app_rx.recv().expect("receive forwarded event");
        assert!(matches!(
            forwarded,
            AppEventPayload::IndexingProgress {
                current: 3,
                total: 5
            }
        ));
        assert!(app_rx.try_recv().is_err());
        handle.join().expect("join forwarder");
    }
}
