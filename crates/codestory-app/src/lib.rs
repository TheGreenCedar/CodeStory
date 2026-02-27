use codestory_api::{
    AgentAnswerDto, AgentAskRequest, AgentBackend, AgentConnectionSettingsDto, ApiError,
    AppEventPayload, BookmarkCategoryDto, BookmarkDto, CreateBookmarkCategoryRequest,
    CreateBookmarkRequest, EdgeId, EdgeKind, EdgeOccurrencesRequest, GraphEdgeDto, GraphNodeDto,
    GraphRequest, GraphResponse, IndexMode, IndexingPhaseTimings, ListChildrenSymbolsRequest,
    ListRootSymbolsRequest, MemberAccess, NodeDetailsDto, NodeDetailsRequest, NodeId, NodeKind,
    NodeOccurrencesRequest, OpenContainingFolderRequest, OpenDefinitionRequest, OpenProjectRequest,
    ProjectSummary, ReadFileTextRequest, ReadFileTextResponse, SearchHit, SearchRequest,
    SetUiLayoutRequest, SourceOccurrenceDto, StartIndexingRequest, StorageStatsDto,
    SymbolSummaryDto, SystemActionResponse, TrailConfigDto, TrailFilterOptionsDto,
    UpdateBookmarkCategoryRequest, UpdateBookmarkRequest, WriteFileResponse, WriteFileTextRequest,
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

mod agent;
mod graph_builders;
mod graph_canonical;
mod path_resolution;
mod system_actions;

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

#[derive(Debug, Clone)]
struct FocusedSourceContext {
    path: String,
    line: u32,
    snippet: String,
}

#[derive(Debug, Clone)]
struct LocalAgentResponse {
    backend_label: &'static str,
    command: String,
    markdown: String,
}

fn agent_backend_label(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Codex => "Codex",
        AgentBackend::ClaudeCode => "Claude Code",
    }
}

fn default_agent_command(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Codex => {
            if cfg!(target_os = "windows") {
                "codex.cmd"
            } else {
                "codex"
            }
        }
        AgentBackend::ClaudeCode => "claude",
    }
}

fn configured_agent_command(connection: &AgentConnectionSettingsDto) -> String {
    connection
        .command
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| default_agent_command(connection.backend).to_string())
}

fn resolve_agent_command(command: &str) -> String {
    if !cfg!(target_os = "windows") {
        return command.to_string();
    }

    if command.contains('\\') || command.contains('/') {
        return command.to_string();
    }

    let mut candidates = Vec::new();
    if let Ok(app_data) = std::env::var("APPDATA") {
        let npm_bin = PathBuf::from(app_data).join("npm");
        candidates.push(npm_bin.join(format!("{command}.cmd")));
        candidates.push(npm_bin.join(format!("{command}.exe")));
        candidates.push(npm_bin.join(command));
    }
    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        let local_bin = PathBuf::from(user_profile).join(".local").join("bin");
        candidates.push(local_bin.join(format!("{command}.exe")));
        candidates.push(local_bin.join(format!("{command}.cmd")));
        candidates.push(local_bin.join(command));
    }
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let windows_apps = PathBuf::from(local_app_data)
            .join("Microsoft")
            .join("WindowsApps");
        candidates.push(windows_apps.join(format!("{command}.exe")));
        candidates.push(windows_apps.join(command));
    }

    for candidate in candidates {
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
    }

    command.to_string()
}

fn truncate_for_diagnostic(raw: &str, max_chars: usize) -> String {
    let mut compact = raw.trim().replace('\r', "");
    if compact.len() > max_chars {
        compact.truncate(max_chars);
        compact.push_str("...");
    }
    compact
}

fn build_local_agent_prompt(
    user_prompt: &str,
    hits: &[SearchHit],
    focused_node: Option<&NodeDetailsDto>,
    focused_source: Option<&FocusedSourceContext>,
) -> String {
    let mut out = String::new();
    out.push_str("You are a codebase assistant. Use only the provided indexed context.\n");
    out.push_str("Do not run tools or execute commands. If context is insufficient, say so.\n\n");
    let _ = writeln!(out, "User request:\n{}\n", user_prompt.trim());

    out.push_str("Indexed symbol hits:\n");
    if hits.is_empty() {
        out.push_str("- none\n");
    } else {
        for hit in hits.iter().take(8) {
            let location = match (&hit.file_path, hit.line) {
                (Some(path), Some(line)) => format!(" ({path}:{line})"),
                (Some(path), None) => format!(" ({path})"),
                _ => String::new(),
            };
            let _ = writeln!(
                out,
                "- {} [{:?}] score {:.3}{}",
                hit.display_name, hit.kind, hit.score, location
            );
        }
    }

    if let Some(node) = focused_node {
        let _ = writeln!(
            out,
            "\nFocused symbol:\n- {} [{:?}]",
            node.display_name, node.kind
        );
        if let Some(path) = node.file_path.as_deref() {
            let _ = writeln!(out, "- file: {}", path);
        }
        if let Some(line) = node.start_line {
            let _ = writeln!(out, "- start line: {}", line);
        }
    }

    if let Some(source) = focused_source {
        let _ = writeln!(
            out,
            "\nSource snippet from {}:{}:\n{}",
            source.path, source.line, source.snippet
        );
    }

    out.push_str(
        "\nRespond in markdown with:\n1. Summary\n2. Key findings\n3. Recommended next steps\n",
    );
    out
}

#[derive(Debug, Clone, Default)]
struct OptionalResolutionTelemetry {
    resolution_unresolved_counts_ms: Option<u32>,
    resolution_calls_ms: Option<u32>,
    resolution_imports_ms: Option<u32>,
    resolution_cleanup_ms: Option<u32>,
    resolved_calls_same_file: Option<u32>,
    resolved_calls_same_module: Option<u32>,
    resolved_calls_global_unique: Option<u32>,
    resolved_calls_semantic: Option<u32>,
    resolved_imports_same_file: Option<u32>,
    resolved_imports_same_module: Option<u32>,
    resolved_imports_global_unique: Option<u32>,
    resolved_imports_fuzzy: Option<u32>,
    resolved_imports_semantic: Option<u32>,
}

impl OptionalResolutionTelemetry {
    fn from_incremental_stats(index_stats: &codestory_index::IncrementalIndexingStats) -> Self {
        if !index_stats.resolution_ran {
            return Self::default();
        }
        Self {
            resolution_unresolved_counts_ms: Some(clamp_u64_to_u32(
                index_stats.resolution_unresolved_counts_ms,
            )),
            resolution_calls_ms: Some(clamp_u64_to_u32(index_stats.resolution_calls_ms)),
            resolution_imports_ms: Some(clamp_u64_to_u32(index_stats.resolution_imports_ms)),
            resolution_cleanup_ms: Some(clamp_u64_to_u32(index_stats.resolution_cleanup_ms)),
            resolved_calls_same_file: Some(clamp_usize_to_u32(
                index_stats.resolved_calls_same_file,
            )),
            resolved_calls_same_module: Some(clamp_usize_to_u32(
                index_stats.resolved_calls_same_module,
            )),
            resolved_calls_global_unique: Some(clamp_usize_to_u32(
                index_stats.resolved_calls_global_unique,
            )),
            resolved_calls_semantic: Some(clamp_usize_to_u32(index_stats.resolved_calls_semantic)),
            resolved_imports_same_file: Some(clamp_usize_to_u32(
                index_stats.resolved_imports_same_file,
            )),
            resolved_imports_same_module: Some(clamp_usize_to_u32(
                index_stats.resolved_imports_same_module,
            )),
            resolved_imports_global_unique: Some(clamp_usize_to_u32(
                index_stats.resolved_imports_global_unique,
            )),
            resolved_imports_fuzzy: Some(clamp_usize_to_u32(index_stats.resolved_imports_fuzzy)),
            resolved_imports_semantic: Some(clamp_usize_to_u32(
                index_stats.resolved_imports_semantic,
            )),
        }
    }
}

fn parse_db_id(raw: &str, field_name: &str) -> Result<i64, ApiError> {
    raw.trim()
        .parse::<i64>()
        .map_err(|_| ApiError::invalid_argument(format!("Invalid {field_name}: {raw}")))
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

fn member_access_dto(access: Option<codestory_core::AccessKind>) -> Option<MemberAccess> {
    access.map(MemberAccess::from)
}

fn status_response(message: impl Into<String>) -> SystemActionResponse {
    SystemActionResponse {
        ok: true,
        message: message.into(),
    }
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
        path_resolution::resolve_project_file_path(self, path, allow_missing_leaf)
    }

    fn open_folder_in_os(path: &Path) -> io::Result<()> {
        system_actions::open_folder_in_os(path)
    }

    fn run_local_agent(
        &self,
        connection: &AgentConnectionSettingsDto,
        prompt: &str,
    ) -> Result<LocalAgentResponse, ApiError> {
        agent::local_runner::run_local_agent(self, connection, prompt)
    }

    fn launch_definition_in_ide(
        &self,
        path: &Path,
        line: Option<u32>,
        col: Option<u32>,
    ) -> Result<SystemActionResponse, ApiError> {
        system_actions::launch_definition_in_ide(path, line, col)
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
        agent::agent_ask(self, req)
    }

    pub fn graph_neighborhood(&self, req: GraphRequest) -> Result<GraphResponse, ApiError> {
        graph_builders::graph_neighborhood(self, req)
    }

    pub fn graph_trail(&self, req: TrailConfigDto) -> Result<GraphResponse, ApiError> {
        graph_builders::graph_trail(self, req)
    }

    pub fn graph_trail_filter_options(&self) -> Result<TrailFilterOptionsDto, ApiError> {
        let storage = self.open_storage()?;
        let node_kinds = storage
            .get_present_node_kinds()
            .map_err(|e| ApiError::internal(format!("Failed to load node kinds: {e}")))?
            .into_iter()
            .map(NodeKind::from)
            .collect::<Vec<_>>();
        let edge_kinds = storage
            .get_present_edge_kinds()
            .map_err(|e| ApiError::internal(format!("Failed to load edge kinds: {e}")))?
            .into_iter()
            .map(EdgeKind::from)
            .collect::<Vec<_>>();
        Ok(TrailFilterOptionsDto {
            node_kinds,
            edge_kinds,
        })
    }

    pub fn list_bookmark_categories(&self) -> Result<Vec<BookmarkCategoryDto>, ApiError> {
        let storage = self.open_storage()?;
        let categories = storage
            .get_bookmark_categories()
            .map_err(|e| ApiError::internal(format!("Failed to load bookmark categories: {e}")))?;
        Ok(categories
            .into_iter()
            .map(|category| BookmarkCategoryDto {
                id: category.id.to_string(),
                name: category.name,
            })
            .collect())
    }

    pub fn create_bookmark_category(
        &self,
        req: CreateBookmarkCategoryRequest,
    ) -> Result<BookmarkCategoryDto, ApiError> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(ApiError::invalid_argument(
                "Bookmark category name cannot be empty.",
            ));
        }

        let storage = self.open_storage()?;
        let id = storage
            .create_bookmark_category(name)
            .map_err(|e| ApiError::internal(format!("Failed to create bookmark category: {e}")))?;
        Ok(BookmarkCategoryDto {
            id: id.to_string(),
            name: name.to_string(),
        })
    }

    pub fn update_bookmark_category(
        &self,
        id: i64,
        req: UpdateBookmarkCategoryRequest,
    ) -> Result<BookmarkCategoryDto, ApiError> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(ApiError::invalid_argument(
                "Bookmark category name cannot be empty.",
            ));
        }
        let storage = self.open_storage()?;
        let updated = storage
            .rename_bookmark_category(id, name)
            .map_err(|e| ApiError::internal(format!("Failed to update bookmark category: {e}")))?;
        if !updated {
            return Err(ApiError::not_found(format!(
                "Bookmark category not found: {id}"
            )));
        }
        Ok(BookmarkCategoryDto {
            id: id.to_string(),
            name: name.to_string(),
        })
    }

    pub fn delete_bookmark_category(&self, id: i64) -> Result<(), ApiError> {
        let storage = self.open_storage()?;
        storage
            .delete_bookmark_category(id)
            .map_err(|e| ApiError::internal(format!("Failed to delete bookmark category: {e}")))?;
        Ok(())
    }

    pub fn list_bookmarks(&self, category_id: Option<i64>) -> Result<Vec<BookmarkDto>, ApiError> {
        let storage = self.open_storage()?;
        let bookmarks = storage
            .get_bookmarks(category_id)
            .map_err(|e| ApiError::internal(format!("Failed to load bookmarks: {e}")))?;

        let mut response = Vec::with_capacity(bookmarks.len());
        for bookmark in bookmarks {
            let node = storage
                .get_node(bookmark.node_id)
                .map_err(|e| ApiError::internal(format!("Failed to load bookmark node: {e}")))?;
            let (node_label, node_kind, file_path) = match node {
                Some(node) => (
                    node_display_name(&node),
                    NodeKind::from(node.kind),
                    Self::file_path_for_node(&storage, &node)?,
                ),
                None => (bookmark.node_id.0.to_string(), NodeKind::UNKNOWN, None),
            };
            response.push(BookmarkDto {
                id: bookmark.id.to_string(),
                category_id: bookmark.category_id.to_string(),
                node_id: NodeId::from(bookmark.node_id),
                comment: bookmark.comment,
                node_label,
                node_kind,
                file_path,
            });
        }
        Ok(response)
    }

    pub fn create_bookmark(&self, req: CreateBookmarkRequest) -> Result<BookmarkDto, ApiError> {
        let node_id = req.node_id.to_core()?;
        let category_id = parse_db_id(&req.category_id, "category_id")?;
        let storage = self.open_storage()?;
        let node = storage
            .get_node(node_id)
            .map_err(|e| ApiError::internal(format!("Failed to load bookmark node: {e}")))?
            .ok_or_else(|| ApiError::not_found(format!("Node not found: {}", req.node_id.0)))?;
        let bookmark_id = storage
            .add_bookmark(category_id, node_id, req.comment.as_deref())
            .map_err(|e| ApiError::internal(format!("Failed to create bookmark: {e}")))?;

        Ok(BookmarkDto {
            id: bookmark_id.to_string(),
            category_id: category_id.to_string(),
            node_id: NodeId::from(node_id),
            comment: req.comment,
            node_label: node_display_name(&node),
            node_kind: NodeKind::from(node.kind),
            file_path: Self::file_path_for_node(&storage, &node)?,
        })
    }

    pub fn update_bookmark(
        &self,
        id: i64,
        req: UpdateBookmarkRequest,
    ) -> Result<BookmarkDto, ApiError> {
        let storage = self.open_storage()?;
        let category_id = req
            .category_id
            .as_deref()
            .map(|raw| parse_db_id(raw, "category_id"))
            .transpose()?;
        let comment_patch = req.comment.as_ref().map(|value| value.as_deref());
        storage
            .update_bookmark(id, category_id, comment_patch)
            .map_err(|e| ApiError::internal(format!("Failed to update bookmark: {e}")))?;
        let bookmark = storage
            .get_bookmarks(None)
            .map_err(|e| ApiError::internal(format!("Failed to reload bookmarks: {e}")))?
            .into_iter()
            .find(|bookmark| bookmark.id == id)
            .ok_or_else(|| ApiError::not_found(format!("Bookmark not found: {id}")))?;
        let node = storage
            .get_node(bookmark.node_id)
            .map_err(|e| ApiError::internal(format!("Failed to load bookmark node: {e}")))?;

        let (node_label, node_kind, file_path) = match node {
            Some(node) => (
                node_display_name(&node),
                NodeKind::from(node.kind),
                Self::file_path_for_node(&storage, &node)?,
            ),
            None => (bookmark.node_id.0.to_string(), NodeKind::UNKNOWN, None),
        };

        Ok(BookmarkDto {
            id: bookmark.id.to_string(),
            category_id: bookmark.category_id.to_string(),
            node_id: NodeId::from(bookmark.node_id),
            comment: bookmark.comment,
            node_label,
            node_kind,
            file_path,
        })
    }

    pub fn delete_bookmark(&self, id: i64) -> Result<(), ApiError> {
        let storage = self.open_storage()?;
        storage
            .delete_bookmark(id)
            .map_err(|e| ApiError::internal(format!("Failed to delete bookmark: {e}")))?;
        Ok(())
    }

    pub fn open_definition(
        &self,
        req: OpenDefinitionRequest,
    ) -> Result<SystemActionResponse, ApiError> {
        let node_id = req.node_id.to_core()?;
        let storage = self.open_storage()?;
        let node = storage
            .get_node(node_id)
            .map_err(|e| ApiError::internal(format!("Failed to load node: {e}")))?
            .ok_or_else(|| ApiError::not_found(format!("Node not found: {}", req.node_id.0)))?;

        let raw_path = if node.kind == codestory_core::NodeKind::FILE {
            Some(node.serialized_name.clone())
        } else {
            Self::file_path_for_node(&storage, &node)?
        }
        .ok_or_else(|| ApiError::invalid_argument("Node has no file path for definition open."))?;

        let resolved = self.resolve_project_file_path(&raw_path, false)?;
        self.launch_definition_in_ide(&resolved, node.start_line, node.start_col)
    }

    pub fn open_containing_folder(
        &self,
        req: OpenContainingFolderRequest,
    ) -> Result<SystemActionResponse, ApiError> {
        let resolved = self.resolve_project_file_path(&req.path, false)?;
        Self::open_folder_in_os(&resolved).map_err(|e| {
            ApiError::internal(format!(
                "Failed to open containing folder for {}: {e}",
                resolved.display()
            ))
        })?;
        Ok(status_response(format!(
            "Opened containing folder for {}",
            resolved.display()
        )))
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
            member_access: member_access_dto(storage.get_component_access(node.id).ok().flatten()),
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
    let resolution_telemetry = OptionalResolutionTelemetry::from_incremental_stats(&index_stats);
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
            resolution_unresolved_counts_ms: resolution_telemetry.resolution_unresolved_counts_ms,
            resolution_calls_ms: resolution_telemetry.resolution_calls_ms,
            resolution_imports_ms: resolution_telemetry.resolution_imports_ms,
            resolution_cleanup_ms: resolution_telemetry.resolution_cleanup_ms,
            resolved_calls_same_file: resolution_telemetry.resolved_calls_same_file,
            resolved_calls_same_module: resolution_telemetry.resolved_calls_same_module,
            resolved_calls_global_unique: resolution_telemetry.resolved_calls_global_unique,
            resolved_calls_semantic: resolution_telemetry.resolved_calls_semantic,
            resolved_imports_same_file: resolution_telemetry.resolved_imports_same_file,
            resolved_imports_same_module: resolution_telemetry.resolved_imports_same_module,
            resolved_imports_global_unique: resolution_telemetry.resolved_imports_global_unique,
            resolved_imports_fuzzy: resolution_telemetry.resolved_imports_fuzzy,
            resolved_imports_semantic: resolution_telemetry.resolved_imports_semantic,
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
        assert!(
            graph.canonical_layout.is_some(),
            "Expected canonical_layout on neighborhood response"
        );
    }

    #[test]
    fn graph_trail_includes_canonical_layout() {
        let temp = tempdir().expect("create temp dir");
        let db_path = temp.path().join("codestory.db");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(1),
                        kind: NodeKind::CLASS,
                        serialized_name: "Runner".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(2),
                        kind: NodeKind::METHOD,
                        serialized_name: "Runner::run".to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(3),
                        kind: NodeKind::METHOD,
                        serialized_name: "Worker::execute".to_string(),
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
                        source: CoreNodeId(2),
                        target: CoreNodeId(3),
                        kind: EdgeKind::CALL,
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
            .graph_trail(TrailConfigDto {
                root_id: codestory_api::NodeId("2".to_string()),
                mode: codestory_api::TrailMode::Neighborhood,
                target_id: None,
                depth: 2,
                direction: codestory_api::TrailDirection::Both,
                caller_scope: codestory_api::TrailCallerScope::ProductionOnly,
                edge_filter: vec![],
                show_utility_calls: false,
                node_filter: vec![],
                max_nodes: 128,
                layout_direction: codestory_api::LayoutDirection::Horizontal,
            })
            .expect("load graph trail");

        assert!(
            graph.canonical_layout.is_some(),
            "Expected canonical_layout on trail response"
        );
    }

    #[test]
    fn update_bookmark_category_returns_not_found_when_missing() {
        let temp = tempdir().expect("create temp dir");
        let controller = AppController::new();
        controller
            .open_project(OpenProjectRequest {
                path: temp.path().to_string_lossy().to_string(),
            })
            .expect("open project");

        let err = controller
            .update_bookmark_category(
                9_999,
                UpdateBookmarkCategoryRequest {
                    name: "Renamed".to_string(),
                },
            )
            .expect_err("missing category should return not_found");

        assert_eq!(err.code, "not_found");
    }
}
