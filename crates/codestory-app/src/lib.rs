use codestory_api::{
    AgentAnswerDto, AgentAskRequest, AgentCitationDto, AgentResponseSectionDto, ApiError,
    AppEventPayload, EdgeId, EdgeKind, EdgeOccurrencesRequest, GraphArtifactDto, GraphEdgeDto,
    GraphNodeDto, GraphRequest, GraphResponse, IndexMode, IndexingPhaseTimings,
    ListChildrenSymbolsRequest, ListRootSymbolsRequest, NodeDetailsDto, NodeDetailsRequest, NodeId,
    NodeKind, NodeOccurrencesRequest, OpenProjectRequest, ProjectSummary, ReadFileTextRequest,
    ReadFileTextResponse, SearchHit, SearchRequest, SetUiLayoutRequest, SourceOccurrenceDto,
    StartIndexingRequest, StorageStatsDto, SymbolSummaryDto, TrailConfigDto, WriteFileResponse,
    WriteFileTextRequest,
};
use codestory_events::{Event, EventBus};
use codestory_search::SearchEngine;
use codestory_storage::Storage;
use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

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

fn clamp_u64_to_u32(v: u64) -> u32 {
    v.min(u32::MAX as u64) as u32
}

fn clamp_u128_to_u32(v: u128) -> u32 {
    v.min(u32::MAX as u128) as u32
}

fn clamp_usize_to_u32(v: usize) -> u32 {
    v.min(u32::MAX as usize) as u32
}

fn edge_certainty_label(
    certainty: Option<codestory_core::ResolutionCertainty>,
    confidence: Option<f32>,
) -> Option<String> {
    certainty
        .or_else(|| codestory_core::ResolutionCertainty::from_confidence(confidence))
        .map(|value| value.as_str().to_string())
}

fn is_structural_kind(kind: codestory_core::NodeKind) -> bool {
    matches!(
        kind,
        codestory_core::NodeKind::CLASS
            | codestory_core::NodeKind::STRUCT
            | codestory_core::NodeKind::INTERFACE
            | codestory_core::NodeKind::UNION
            | codestory_core::NodeKind::ENUM
            | codestory_core::NodeKind::NAMESPACE
            | codestory_core::NodeKind::MODULE
    )
}

#[derive(Debug, Clone, Copy)]
struct AppGraphFeatureFlags {
    include_edge_certainty: bool,
    include_callsite_identity: bool,
    include_candidate_targets: bool,
}

impl AppGraphFeatureFlags {
    fn from_env() -> Self {
        Self {
            include_edge_certainty: env_flag("CODESTORY_GRAPH_INCLUDE_EDGE_CERTAINTY", true),
            include_callsite_identity: env_flag("CODESTORY_GRAPH_INCLUDE_CALLSITE_IDENTITY", true),
            include_candidate_targets: env_flag("CODESTORY_GRAPH_INCLUDE_CANDIDATE_TARGETS", true),
        }
    }
}

fn app_graph_flags() -> AppGraphFeatureFlags {
    static FLAGS: OnceLock<AppGraphFeatureFlags> = OnceLock::new();
    *FLAGS.get_or_init(AppGraphFeatureFlags::from_env)
}

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim(),
            "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
        ),
        Err(_) => default,
    }
}

fn graph_edge_dto(edge: codestory_core::Edge, flags: AppGraphFeatureFlags) -> GraphEdgeDto {
    GraphEdgeDto {
        id: EdgeId::from(edge.id),
        source: NodeId::from(edge.source),
        target: NodeId::from(edge.target),
        kind: EdgeKind::from(edge.kind),
        confidence: edge.confidence,
        certainty: if flags.include_edge_certainty {
            edge_certainty_label(edge.certainty, edge.confidence)
        } else {
            None
        },
        callsite_identity: if flags.include_callsite_identity {
            edge.callsite_identity.clone()
        } else {
            None
        },
        candidate_targets: if flags.include_candidate_targets {
            edge.candidate_targets
                .iter()
                .copied()
                .map(NodeId::from)
                .collect()
        } else {
            Vec::new()
        },
    }
}

fn sanitize_mermaid_label(input: &str) -> String {
    input.replace('"', "'").replace('\n', " ")
}

fn markdown_snippet(text: &str, focus_line: Option<u32>, context: usize) -> String {
    let all_lines: Vec<&str> = text.lines().collect();
    if all_lines.is_empty() {
        return String::new();
    }

    let line_index = focus_line
        .and_then(|line| line.checked_sub(1))
        .map(|line| line as usize)
        .unwrap_or(0)
        .min(all_lines.len().saturating_sub(1));

    let start = line_index.saturating_sub(context);
    let end = (line_index + context + 1).min(all_lines.len());

    let mut out = String::new();
    out.push_str("```text\n");
    for (idx, line) in all_lines[start..end].iter().enumerate() {
        let source_line = start + idx + 1;
        let marker = if source_line == line_index + 1 {
            ">"
        } else {
            " "
        };
        let _ = writeln!(out, "{marker}{source_line:>5} | {line}");
    }
    out.push_str("```");
    out
}

fn mermaid_flowchart(graph: &GraphResponse) -> String {
    let mut out = String::from("flowchart LR\n");
    for node in graph.nodes.iter().take(14) {
        let _ = writeln!(
            out,
            "    N{}[\"{}\"]",
            node.id.0,
            sanitize_mermaid_label(&node.label)
        );
    }

    for edge in graph.edges.iter().take(20) {
        let _ = writeln!(
            out,
            "    N{} -->|\"{:?}\"| N{}",
            edge.source.0, edge.kind, edge.target.0
        );
    }

    out
}

fn mermaid_sequence(graph: &GraphResponse) -> String {
    let mut out = String::from("sequenceDiagram\n");
    let mut labels: HashMap<String, String> = HashMap::new();
    for node in graph.nodes.iter().take(10) {
        labels.insert(node.id.0.clone(), node.label.clone());
    }

    let mut emitted = 0usize;
    for edge in graph.edges.iter().take(14) {
        let Some(source) = labels.get(&edge.source.0) else {
            continue;
        };
        let Some(target) = labels.get(&edge.target.0) else {
            continue;
        };

        emitted += 1;
        let _ = writeln!(
            out,
            "    {}->>{}: {:?}",
            sanitize_mermaid_label(source),
            sanitize_mermaid_label(target),
            edge.kind
        );
    }

    if emitted == 0 {
        out.push_str("    User->>System: No sequencing data available\n");
    }
    out
}

fn mermaid_gantt(citations: &[SearchHit]) -> String {
    let mut out = String::from("gantt\n    title Investigation Plan\n    dateFormat X\n");
    let mut current = 0u32;
    for (idx, hit) in citations.iter().take(5).enumerate() {
        let duration = 1 + (idx as u32 % 2);
        let _ = writeln!(
            out,
            "    {} :{}, {}, {}",
            sanitize_mermaid_label(&hit.display_name),
            idx + 1,
            current,
            duration
        );
        current += duration;
    }
    if citations.is_empty() {
        out.push_str("    Baseline scan :1, 0, 1\n");
    }
    out
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

    fn open_storage(&self) -> Result<Storage, ApiError> {
        let storage_path = self.require_storage_path()?;
        Storage::open(&storage_path)
            .map_err(|e| ApiError::internal(format!("Failed to open storage: {e}")))
    }

    fn resolve_project_file_path(
        &self,
        path: &str,
        allow_missing_leaf: bool,
    ) -> Result<PathBuf, ApiError> {
        let root = self.require_project_root()?;
        let root = root
            .canonicalize()
            .map_err(|e| ApiError::internal(format!("Failed to resolve project root: {e}")))?;

        let raw = PathBuf::from(path);
        let candidate = if raw.is_absolute() {
            raw
        } else {
            root.join(raw)
        };

        let resolved = match candidate.canonicalize() {
            Ok(canonical) => canonical,
            Err(err) if allow_missing_leaf && err.kind() == io::ErrorKind::NotFound => {
                let Some(parent) = candidate.parent() else {
                    return Err(ApiError::invalid_argument(format!(
                        "Invalid file path: {}",
                        candidate.display()
                    )));
                };
                let Some(file_name) = candidate.file_name() else {
                    return Err(ApiError::invalid_argument(format!(
                        "Invalid file path: {}",
                        candidate.display()
                    )));
                };

                let parent = parent.canonicalize().map_err(|e| {
                    if e.kind() == io::ErrorKind::NotFound {
                        ApiError::not_found(format!(
                            "Parent directory not found: {}",
                            parent.display()
                        ))
                    } else {
                        ApiError::internal(format!("Failed to resolve parent path: {e}"))
                    }
                })?;
                parent.join(file_name)
            }
            Err(err) => {
                return Err(if err.kind() == io::ErrorKind::NotFound {
                    ApiError::not_found(format!("File not found: {}", candidate.display()))
                } else {
                    ApiError::internal(format!("Failed to resolve file path: {err}"))
                });
            }
        };

        if !resolved.starts_with(&root) {
            return Err(ApiError::invalid_argument(
                "Refusing to access file outside project root.",
            ));
        }

        Ok(resolved)
    }

    fn cached_labels<I>(&self, ids: I) -> HashMap<codestory_core::NodeId, String>
    where
        I: IntoIterator<Item = codestory_core::NodeId>,
    {
        let s = self.state.lock();
        ids.into_iter()
            .filter_map(|id| s.node_names.get(&id).cloned().map(|name| (id, name)))
            .collect()
    }

    fn file_path_for_node(
        storage: &Storage,
        node: &codestory_core::Node,
    ) -> Result<Option<String>, ApiError> {
        let Some(file_id) = node.file_node_id else {
            return Ok(None);
        };

        let file_node = storage
            .get_node(file_id)
            .map_err(|e| ApiError::internal(format!("Failed to load file node: {e}")))?;

        Ok(file_node.map(|file| file.serialized_name))
    }

    fn occurrence_kind_label(kind: codestory_core::OccurrenceKind) -> &'static str {
        match kind {
            codestory_core::OccurrenceKind::DEFINITION => "definition",
            codestory_core::OccurrenceKind::REFERENCE => "reference",
            codestory_core::OccurrenceKind::DECLARATION => "declaration",
            codestory_core::OccurrenceKind::MACRO_DEFINITION => "macro_definition",
            codestory_core::OccurrenceKind::MACRO_REFERENCE => "macro_reference",
            codestory_core::OccurrenceKind::UNKNOWN => "unknown",
        }
    }

    fn to_source_occurrence_dto(
        storage: &Storage,
        occurrence: codestory_core::Occurrence,
    ) -> Result<Option<SourceOccurrenceDto>, ApiError> {
        let file_node = storage
            .get_node(occurrence.location.file_node_id)
            .map_err(|e| {
                ApiError::internal(format!("Failed to resolve occurrence file node: {e}"))
            })?;
        let Some(file_node) = file_node else {
            return Ok(None);
        };

        Ok(Some(SourceOccurrenceDto {
            element_id: occurrence.element_id.to_string(),
            kind: Self::occurrence_kind_label(occurrence.kind).to_string(),
            file_path: file_node.serialized_name,
            start_line: occurrence.location.start_line,
            start_col: occurrence.location.start_col,
            end_line: occurrence.location.end_line,
            end_col: occurrence.location.end_col,
        }))
    }

    fn symbol_summary_for_node(
        storage: &Storage,
        labels_by_id: &HashMap<codestory_core::NodeId, String>,
        node: codestory_core::Node,
    ) -> Result<SymbolSummaryDto, ApiError> {
        let has_children = !storage
            .get_children_symbols(node.id)
            .map_err(|e| ApiError::internal(format!("Failed to load child symbols: {e}")))?
            .is_empty();

        let label = labels_by_id
            .get(&node.id)
            .cloned()
            .unwrap_or_else(|| node_display_name(&node));

        Ok(SymbolSummaryDto {
            id: NodeId::from(node.id),
            label,
            kind: NodeKind::from(node.kind),
            file_path: Self::file_path_for_node(storage, &node)?,
            has_children,
        })
    }

    fn dedupe_symbol_nodes(
        nodes: Vec<codestory_core::Node>,
        labels_by_id: &HashMap<codestory_core::NodeId, String>,
    ) -> Vec<codestory_core::Node> {
        let mut seen = HashSet::new();
        let mut deduped = Vec::with_capacity(nodes.len());

        for node in nodes {
            let label = labels_by_id
                .get(&node.id)
                .cloned()
                .unwrap_or_else(|| node_display_name(&node));
            let key = (node.kind as i32, label, node.file_node_id);
            if seen.insert(key) {
                deduped.push(node);
            }
        }

        deduped
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
            let indexing_started = std::time::Instant::now();
            let result = match req.mode {
                IndexMode::Full => index_full(&root, &storage_path, &events_tx),
                IndexMode::Incremental => index_incremental(&root, &storage_path, &events_tx),
            };

            match result {
                Ok(mut summary) => {
                    let _ = events_tx.send(AppEventPayload::StatusUpdate {
                        message: "Indexing finished. Refreshing caches...".to_string(),
                    });
                    let cache_started = std::time::Instant::now();
                    if let Ok(storage) = Storage::open(&storage_path) {
                        refresh_caches(&controller, &storage);
                        summary.phase_timings.cache_refresh_ms =
                            Some(clamp_u128_to_u32(cache_started.elapsed().as_millis()));
                    } else {
                        controller.state.lock().is_indexing = false;
                    }

                    let _ = events_tx.send(AppEventPayload::IndexingComplete {
                        duration_ms: clamp_u128_to_u32(indexing_started.elapsed().as_millis()),
                        phase_timings: summary.phase_timings,
                    });
                }
                Err(err) => {
                    let _ = events_tx.send(AppEventPayload::IndexingFailed { error: err.message });
                    if let Ok(storage) = Storage::open(&storage_path) {
                        refresh_caches(&controller, &storage);
                    } else {
                        controller.state.lock().is_indexing = false;
                    }
                }
            }
        });

        Ok(())
    }

    pub fn search(&self, req: SearchRequest) -> Result<Vec<SearchHit>, ApiError> {
        let (matches, node_names) = {
            let mut s = self.state.lock();
            let engine = s.search_engine.as_mut().ok_or_else(|| {
                ApiError::invalid_argument("Search engine not initialized. Open a project first.")
            })?;
            let matches = engine.search_symbol_with_scores(&req.query);
            let node_names = s.node_names.clone();
            (matches, node_names)
        };

        let storage = self.open_storage()?;

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

    pub fn list_root_symbols(
        &self,
        req: ListRootSymbolsRequest,
    ) -> Result<Vec<SymbolSummaryDto>, ApiError> {
        let storage = self.open_storage()?;

        let mut roots = storage
            .get_root_symbols()
            .map_err(|e| ApiError::internal(format!("Failed to load root symbols: {e}")))?;
        roots.sort_by_cached_key(node_display_name);

        let labels_by_id = self.cached_labels(roots.iter().map(|node| node.id));
        roots = Self::dedupe_symbol_nodes(roots, &labels_by_id);

        let limit = req.limit.unwrap_or(300).clamp(1, 2_000) as usize;
        if roots.len() > limit {
            roots.truncate(limit);
        }

        roots
            .into_iter()
            .map(|node| Self::symbol_summary_for_node(&storage, &labels_by_id, node))
            .collect()
    }

    pub fn list_children_symbols(
        &self,
        req: ListChildrenSymbolsRequest,
    ) -> Result<Vec<SymbolSummaryDto>, ApiError> {
        let parent_id = req.parent_id.to_core()?;
        let storage = self.open_storage()?;

        let mut children = storage
            .get_children_symbols(parent_id)
            .map_err(|e| ApiError::internal(format!("Failed to load child symbols: {e}")))?;
        children.sort_by_cached_key(node_display_name);

        let labels_by_id = self.cached_labels(children.iter().map(|node| node.id));
        children = Self::dedupe_symbol_nodes(children, &labels_by_id);
        children
            .into_iter()
            .map(|node| Self::symbol_summary_for_node(&storage, &labels_by_id, node))
            .collect()
    }

    pub fn agent_ask(&self, req: AgentAskRequest) -> Result<AgentAnswerDto, ApiError> {
        let prompt = req.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(ApiError::invalid_argument("Prompt cannot be empty."));
        }

        let mut hits = self.search(SearchRequest {
            query: prompt.clone(),
        })?;
        let max_results = req.max_results.unwrap_or(8).clamp(1, 25) as usize;
        if hits.len() > max_results {
            hits.truncate(max_results);
        }

        let mut chosen_node = req.focus_node_id.clone();
        if chosen_node.is_none() {
            chosen_node = hits.first().map(|hit| hit.node_id.clone());
        }

        let mut graphs = Vec::new();
        let mut primary_graph_id = None::<String>;

        if let Some(center_id) = chosen_node.clone() {
            let neighborhood = self.graph_neighborhood(GraphRequest {
                center_id: center_id.clone(),
                max_edges: Some(240),
            })?;

            primary_graph_id = Some("uml-primary".to_string());
            graphs.push(GraphArtifactDto::Uml {
                id: "uml-primary".to_string(),
                title: "Primary Dependency Graph".to_string(),
                graph: neighborhood.clone(),
            });

            if req.include_mermaid {
                graphs.push(GraphArtifactDto::Mermaid {
                    id: "mermaid-flow".to_string(),
                    title: "Flow Overview".to_string(),
                    diagram: "flowchart".to_string(),
                    mermaid_syntax: mermaid_flowchart(&neighborhood),
                });

                let prompt_lower = prompt.to_ascii_lowercase();
                if prompt_lower.contains("sequence") {
                    graphs.push(GraphArtifactDto::Mermaid {
                        id: "mermaid-sequence".to_string(),
                        title: "Sequence Narrative".to_string(),
                        diagram: "sequenceDiagram".to_string(),
                        mermaid_syntax: mermaid_sequence(&neighborhood),
                    });
                }

                if prompt_lower.contains("timeline") || prompt_lower.contains("gantt") {
                    graphs.push(GraphArtifactDto::Mermaid {
                        id: "mermaid-gantt".to_string(),
                        title: "Execution Timeline".to_string(),
                        diagram: "gantt".to_string(),
                        mermaid_syntax: mermaid_gantt(&hits),
                    });
                }
            }
        }

        let citations: Vec<AgentCitationDto> = hits
            .iter()
            .map(|hit| AgentCitationDto {
                node_id: hit.node_id.clone(),
                display_name: hit.display_name.clone(),
                kind: hit.kind,
                file_path: hit.file_path.clone(),
                line: hit.line,
                score: hit.score,
            })
            .collect();

        let mut sections = Vec::new();
        if !hits.is_empty() {
            let mut markdown = String::from("Top symbol matches from indexed search:\n");
            for hit in hits.iter().take(6) {
                let location = match (&hit.file_path, hit.line) {
                    (Some(path), Some(line)) => format!(" ({path}:{line})"),
                    (Some(path), None) => format!(" ({path})"),
                    _ => String::new(),
                };
                let _ = writeln!(
                    markdown,
                    "- **{}** [{:?}] score `{:.3}`{}",
                    hit.display_name, hit.kind, hit.score, location
                );
            }
            sections.push(AgentResponseSectionDto {
                id: "search-results".to_string(),
                title: "Indexed Search".to_string(),
                markdown,
                graph_ids: Vec::new(),
            });
        } else {
            sections.push(AgentResponseSectionDto {
                id: "search-results".to_string(),
                title: "Indexed Search".to_string(),
                markdown: "No direct indexed matches were found. Try a symbol name, file stem, or a narrower phrase.".to_string(),
                graph_ids: Vec::new(),
            });
        }

        if let Some(center_id) = chosen_node {
            let node = self.node_details(NodeDetailsRequest {
                id: center_id.clone(),
            })?;

            let mut markdown = format!(
                "Focused symbol: **{}** (`{:?}`)\n",
                node.display_name, node.kind
            );

            if let (Some(path), Some(line)) = (node.file_path.clone(), node.start_line)
                && let Ok(file) = self.read_file_text(ReadFileTextRequest { path: path.clone() })
            {
                let _ = writeln!(markdown, "\nSource context from `{path}:{line}`:\n");
                markdown.push_str(&markdown_snippet(&file.text, Some(line), 6));
            }

            let graph_ids = primary_graph_id.clone().into_iter().collect();
            sections.push(AgentResponseSectionDto {
                id: "deep-inspection".to_string(),
                title: "Deep Inspection".to_string(),
                markdown,
                graph_ids,
            });
        }

        if req.include_mermaid {
            let graph_ids = graphs
                .iter()
                .filter_map(|graph| match graph {
                    GraphArtifactDto::Mermaid { id, .. } => Some(id.clone()),
                    GraphArtifactDto::Uml { .. } => None,
                })
                .collect::<Vec<_>>();

            if !graph_ids.is_empty() {
                sections.push(AgentResponseSectionDto {
                    id: "agent-diagrams".to_string(),
                    title: "Agent Diagrams".to_string(),
                    markdown: "Generated Mermaid diagrams for storytelling views (flow, sequence, or timeline when requested).".to_string(),
                    graph_ids,
                });
            }
        }

        let summary = if hits.is_empty() {
            "No indexed symbols matched the prompt yet.".to_string()
        } else {
            format!(
                "Investigated {} symbol matches and generated {} graph artifact(s).",
                hits.len(),
                graphs.len()
            )
        };

        Ok(AgentAnswerDto {
            prompt,
            summary,
            sections,
            citations,
            graphs,
        })
    }

    pub fn graph_neighborhood(&self, req: GraphRequest) -> Result<GraphResponse, ApiError> {
        let center = req.center_id.to_core()?;
        let graph_flags = app_graph_flags();

        let storage = self.open_storage()?;

        let max_edges = req.max_edges.unwrap_or(400).min(2_000) as usize;
        let mut edges = storage
            .get_edges_for_node_id(center)
            .map_err(|e| ApiError::internal(format!("Failed to load edges: {e}")))?;

        // If the center is a member (for example, a trait method), pull
        // implementation-style edges from its owner node too.
        if let Ok(Some(center_node)) = storage.get_node(center)
            && !is_structural_kind(center_node.kind)
        {
            let mut owner_ids = HashSet::new();
            for edge in &edges {
                if edge.kind != codestory_core::EdgeKind::MEMBER {
                    continue;
                }
                let (source, target) = edge.effective_endpoints();
                if target == center {
                    owner_ids.insert(source);
                }
            }

            for owner_id in owner_ids {
                let owner_edges = storage
                    .get_edges_for_node_id(owner_id)
                    .map_err(|e| ApiError::internal(format!("Failed to load edges: {e}")))?;
                for edge in owner_edges {
                    if matches!(
                        edge.kind,
                        codestory_core::EdgeKind::INHERITANCE | codestory_core::EdgeKind::OVERRIDE
                    ) {
                        edges.push(edge);
                    }
                }
            }
        }

        let mut seen_edge_ids = HashSet::new();
        edges.retain(|edge| seen_edge_ids.insert(edge.id));
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

            edge_dtos.push(graph_edge_dto(edge, graph_flags));

            if seen.insert(source) {
                ordered_node_ids.push(source);
            }
            if seen.insert(target) {
                ordered_node_ids.push(target);
            }
        }

        let mut node_dtos = Vec::with_capacity(ordered_node_ids.len());
        for id in ordered_node_ids {
            let (label, kind, file_path, qualified_name) = match storage.get_node(id) {
                Ok(Some(node)) => (
                    node_display_name(&node),
                    NodeKind::from(node.kind),
                    Self::file_path_for_node(&storage, &node).ok().flatten(),
                    node.qualified_name,
                ),
                _ => (id.0.to_string(), NodeKind::UNKNOWN, None, None),
            };

            node_dtos.push(GraphNodeDto {
                id: NodeId::from(id),
                label,
                kind,
                depth: if id == center { 0 } else { 1 },
                label_policy: Some("qualified_or_serialized".to_string()),
                badge_visible_members: None,
                badge_total_members: None,
                merged_symbol_examples: Vec::new(),
                file_path,
                qualified_name,
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
        let root_id = req.root_id.to_core()?;
        let graph_flags = app_graph_flags();
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
            caller_scope: req.caller_scope.into(),
            edge_filter: req.edge_filter.into_iter().map(Into::into).collect(),
            show_utility_calls: req.show_utility_calls,
            node_filter: req.node_filter.into_iter().map(Into::into).collect(),
            max_nodes: req.max_nodes.clamp(10, 100_000) as usize,
        };

        let storage = self.open_storage()?;

        let result = storage
            .get_trail(&config)
            .map_err(|e| ApiError::internal(format!("Failed to compute trail: {e}")))?;

        let codestory_core::TrailResult {
            nodes,
            edges,
            depth_map,
            truncated,
        } = result;

        let node_kind_by_id: HashMap<codestory_core::NodeId, codestory_core::NodeKind> =
            nodes.iter().map(|node| (node.id, node.kind)).collect();
        let mut visible_member_counts: HashMap<codestory_core::NodeId, u32> = HashMap::new();
        for edge in &edges {
            if edge.kind != codestory_core::EdgeKind::MEMBER {
                continue;
            }
            let (source, target) = edge.effective_endpoints();
            let source_kind = node_kind_by_id.get(&source).copied();
            let target_kind = node_kind_by_id.get(&target).copied();
            let source_is_structural = source_kind.is_some_and(is_structural_kind);
            let target_is_structural = target_kind.is_some_and(is_structural_kind);
            let host_id = if source_is_structural && !target_is_structural {
                Some(source)
            } else if target_is_structural && !source_is_structural {
                Some(target)
            } else {
                None
            };
            if let Some(host_id) = host_id {
                *visible_member_counts.entry(host_id).or_insert(0) += 1;
            }
        }

        let mut node_dtos = Vec::with_capacity(nodes.len());
        for node in nodes {
            let label = node_display_name(&node);
            let depth = depth_map.get(&node.id).copied().unwrap_or(0);
            let is_structural = is_structural_kind(node.kind);
            let badge_visible_members = if is_structural {
                Some(*visible_member_counts.get(&node.id).unwrap_or(&0))
            } else {
                None
            };
            let badge_total_members = if is_structural {
                storage
                    .get_children_symbols(node.id)
                    .ok()
                    .map(|children| children.len() as u32)
            } else {
                None
            };

            node_dtos.push(GraphNodeDto {
                id: NodeId::from(node.id),
                label,
                kind: NodeKind::from(node.kind),
                depth,
                label_policy: Some("qualified_or_serialized".to_string()),
                badge_visible_members,
                badge_total_members,
                merged_symbol_examples: Vec::new(),
                file_path: Self::file_path_for_node(&storage, &node)?,
                qualified_name: node.qualified_name.clone(),
            });
        }

        let mut edge_dtos = Vec::with_capacity(edges.len());
        for edge in edges {
            let edge = edge.with_effective_endpoints();
            edge_dtos.push(graph_edge_dto(edge, graph_flags));
        }

        Ok(GraphResponse {
            center_id: NodeId::from(config.root_id),
            nodes: node_dtos,
            edges: edge_dtos,
            truncated,
        })
    }

    pub fn node_details(&self, req: NodeDetailsRequest) -> Result<NodeDetailsDto, ApiError> {
        let id = req.id.to_core()?;

        let storage = self.open_storage()?;

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

    pub fn node_occurrences(
        &self,
        req: NodeOccurrencesRequest,
    ) -> Result<Vec<SourceOccurrenceDto>, ApiError> {
        let id = req.id.to_core()?;
        let storage = self.open_storage()?;
        let mut occurrences = storage
            .get_occurrences_for_node(id)
            .map_err(|e| ApiError::internal(format!("Failed to load node occurrences: {e}")))?
            .into_iter()
            .filter_map(|occurrence| {
                Self::to_source_occurrence_dto(&storage, occurrence).transpose()
            })
            .collect::<Result<Vec<_>, ApiError>>()?;

        occurrences.sort_by(|left, right| {
            left.file_path
                .cmp(&right.file_path)
                .then(left.start_line.cmp(&right.start_line))
                .then(left.start_col.cmp(&right.start_col))
                .then(left.end_line.cmp(&right.end_line))
                .then(left.end_col.cmp(&right.end_col))
        });
        Ok(occurrences)
    }

    pub fn edge_occurrences(
        &self,
        req: EdgeOccurrencesRequest,
    ) -> Result<Vec<SourceOccurrenceDto>, ApiError> {
        let id = req.id.to_core()?;
        let storage = self.open_storage()?;
        let mut occurrences = storage
            .get_occurrences_for_element(id.0)
            .map_err(|e| ApiError::internal(format!("Failed to load edge occurrences: {e}")))?
            .into_iter()
            .filter_map(|occurrence| {
                Self::to_source_occurrence_dto(&storage, occurrence).transpose()
            })
            .collect::<Result<Vec<_>, ApiError>>()?;

        occurrences.sort_by(|left, right| {
            left.file_path
                .cmp(&right.file_path)
                .then(left.start_line.cmp(&right.start_line))
                .then(left.start_col.cmp(&right.start_col))
                .then(left.end_line.cmp(&right.end_line))
                .then(left.end_col.cmp(&right.end_col))
        });
        Ok(occurrences)
    }

    pub fn read_file_text(
        &self,
        req: ReadFileTextRequest,
    ) -> Result<ReadFileTextResponse, ApiError> {
        let candidate = self.resolve_project_file_path(&req.path, false)?;

        let text = std::fs::read_to_string(&candidate).map_err(|e| {
            ApiError::internal(format!("Failed to read file {}: {e}", candidate.display()))
        })?;

        Ok(ReadFileTextResponse {
            path: candidate.to_string_lossy().to_string(),
            text,
        })
    }

    pub fn write_file_text(
        &self,
        req: WriteFileTextRequest,
    ) -> Result<WriteFileResponse, ApiError> {
        let candidate = self.resolve_project_file_path(&req.path, true)?;
        std::fs::write(&candidate, &req.text).map_err(|e| {
            ApiError::internal(format!("Failed to write file {}: {e}", candidate.display()))
        })?;

        Ok(WriteFileResponse {
            bytes_written: clamp_i64_to_u32(req.text.len() as i64),
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

#[derive(Debug, Clone)]
struct IndexingRunSummary {
    phase_timings: IndexingPhaseTimings,
}

fn index_full(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
) -> Result<IndexingRunSummary, ApiError> {
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
) -> Result<IndexingRunSummary, ApiError> {
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
            match ev {
                Event::IndexingProgress { current, total } => {
                    let _ = progress_tx.send(AppEventPayload::IndexingProgress {
                        current: current.min(u32::MAX as usize) as u32,
                        total: total.min(u32::MAX as usize) as u32,
                    });
                }
                Event::StatusUpdate { message } => {
                    let _ = progress_tx.send(AppEventPayload::StatusUpdate { message });
                }
                _ => {}
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
) -> Result<IndexingRunSummary, ApiError>
where
    F: FnOnce(
        &codestory_project::Project,
        &Storage,
    ) -> Result<codestory_project::RefreshInfo, ApiError>,
{
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

    let index_stats = result.map_err(|e| ApiError::internal(format!("Indexing failed: {e}")))?;
    Ok(IndexingRunSummary {
        phase_timings: IndexingPhaseTimings {
            parse_index_ms: clamp_u64_to_u32(index_stats.parse_index_ms),
            projection_flush_ms: clamp_u64_to_u32(index_stats.projection_flush_ms),
            edge_resolution_ms: clamp_u64_to_u32(index_stats.edge_resolution_ms),
            error_flush_ms: clamp_u64_to_u32(index_stats.error_flush_ms),
            cleanup_ms: clamp_u64_to_u32(index_stats.cleanup_ms),
            cache_refresh_ms: None,
            unresolved_calls_start: clamp_usize_to_u32(index_stats.unresolved_calls_start),
            unresolved_imports_start: clamp_usize_to_u32(index_stats.unresolved_imports_start),
            resolved_calls: clamp_usize_to_u32(index_stats.resolved_calls),
            resolved_imports: clamp_usize_to_u32(index_stats.resolved_imports),
            unresolved_calls_end: clamp_usize_to_u32(index_stats.unresolved_calls_end),
            unresolved_imports_end: clamp_usize_to_u32(index_stats.unresolved_imports_end),
        },
    })
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
    use codestory_core::{Edge, EdgeId, EdgeKind, Node, NodeId as CoreNodeId, NodeKind};
    use crossbeam_channel::unbounded;
    use tempfile::tempdir;

    #[test]
    fn build_search_state_prefers_qualified_name() {
        let nodes = vec![Node {
            id: CoreNodeId(1),
            kind: NodeKind::FUNCTION,
            serialized_name: "short_name".to_string(),
            qualified_name: Some("pkg.mod.short_name".to_string()),
            ..Default::default()
        }];

        let (node_names, mut engine) = build_search_state(nodes).expect("build search state");
        assert_eq!(
            node_names.get(&CoreNodeId(1)).map(String::as_str),
            Some("pkg.mod.short_name")
        );

        let hits = engine.search_symbol("pkg.mod");
        assert_eq!(hits.first().copied(), Some(CoreNodeId(1)));
    }

    #[test]
    fn progress_forwarder_relays_progress_and_status_events() {
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
        let status = app_rx.recv().expect("receive status update");
        assert!(matches!(
            status,
            AppEventPayload::StatusUpdate { message } if message == "ignore me"
        ));
        assert!(
            app_rx.try_recv().is_err(),
            "unexpected extra forwarded events"
        );
        handle.join().expect("join forwarder");
    }

    #[test]
    fn write_file_text_writes_inside_project_root() {
        let temp = tempdir().expect("create temp dir");
        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let result = controller
            .write_file_text(WriteFileTextRequest {
                path: "notes.txt".to_string(),
                text: "hello world".to_string(),
            })
            .expect("write text file");

        assert_eq!(result.bytes_written, 11);
        let saved = std::fs::read_to_string(temp.path().join("notes.txt")).expect("read file");
        assert_eq!(saved, "hello world");
    }

    #[test]
    fn write_file_text_rejects_paths_outside_project_root() {
        let temp = tempdir().expect("create temp dir");
        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let err = controller
            .write_file_text(WriteFileTextRequest {
                path: "../escape.txt".to_string(),
                text: "nope".to_string(),
            })
            .expect_err("write should fail");

        assert_eq!(err.code, "invalid_argument");
    }

    #[test]
    fn list_root_symbols_deduplicates_repeated_entries() {
        let temp = tempdir().expect("create temp dir");
        let db_path = temp.path().join("codestory.db");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(101),
                        kind: NodeKind::MODULE,
                        serialized_name: "\"react\"".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(102),
                        kind: NodeKind::MODULE,
                        serialized_name: "\"react\"".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(103),
                        kind: NodeKind::MODULE,
                        serialized_name: "\"./app/types\"".to_string(),
                        ..Default::default()
                    },
                ])
                .expect("insert root nodes");
        }

        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let roots = controller
            .list_root_symbols(ListRootSymbolsRequest { limit: None })
            .expect("load roots");
        let react_count = roots
            .iter()
            .filter(|symbol| symbol.label == "\"react\"")
            .count();

        assert_eq!(react_count, 1);
        assert!(roots.iter().any(|symbol| symbol.label == "\"./app/types\""));
    }

    #[test]
    fn graph_neighborhood_member_includes_owner_inheritance_edges() {
        let temp = tempdir().expect("create temp dir");
        let db_path = temp.path().join("codestory.db");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(1),
                        kind: NodeKind::INTERFACE,
                        serialized_name: "EventListener".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(2),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "EventListener::handle_event".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(3),
                        kind: NodeKind::CLASS,
                        serialized_name: "UiListener".to_string(),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
            storage
                .insert_edges_batch(&[
                    Edge {
                        id: EdgeId(11),
                        source: CoreNodeId(1),
                        target: CoreNodeId(2),
                        kind: EdgeKind::MEMBER,
                        ..Default::default()
                    },
                    Edge {
                        id: EdgeId(12),
                        source: CoreNodeId(3),
                        target: CoreNodeId(1),
                        kind: EdgeKind::INHERITANCE,
                        ..Default::default()
                    },
                ])
                .expect("insert edges");
        }

        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let graph = controller
            .graph_neighborhood(GraphRequest {
                center_id: codestory_api::NodeId("2".to_string()),
                max_edges: None,
            })
            .expect("load graph neighborhood");

        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.kind == codestory_api::EdgeKind::INHERITANCE),
            "Expected INHERITANCE edge from owner trait context"
        );
    }
}
