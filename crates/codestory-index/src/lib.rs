use anyhow::{Result, anyhow};
use codestory_core::{
    AccessKind, CallableProjectionState, Edge, EdgeId, EdgeKind, Node, NodeId, NodeKind,
    Occurrence, SourceLocation,
};
use codestory_storage::Storage;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use codestory_events::{Event, EventBus};
use rayon::prelude::*;
use streaming_iterator::StreamingIterator;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;
use tree_sitter::{Language, Node as TsNode, Parser, Point, Query, QueryCursor, Tree};
use tree_sitter_graph::ast::File as GraphFile;
use tree_sitter_graph::functions::Functions;
use tree_sitter_graph::{ExecutionConfig, NoCancellation, Variables};

pub mod cancellation;
pub mod compilation_database;
pub mod intermediate_storage;
pub mod resolution;
pub mod semantic;
pub mod symbol_table;
pub use cancellation::CancellationToken;
use intermediate_storage::IntermediateStorage;
use symbol_table::SymbolTable;

#[derive(Debug, Clone, Copy)]
struct IndexFeatureFlags {
    legacy_edge_identity: bool,
}

impl IndexFeatureFlags {
    fn from_env() -> Self {
        Self {
            legacy_edge_identity: env_flag("CODESTORY_INDEX_LEGACY_EDGE_IDENTITY", false)
                || env_flag("CODESTORY_INDEX_LEGACY_DEDUP", false),
        }
    }
}

fn index_feature_flags() -> IndexFeatureFlags {
    static FLAGS: OnceLock<IndexFeatureFlags> = OnceLock::new();
    *FLAGS.get_or_init(IndexFeatureFlags::from_env)
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

// Source of truth for live rule assets. Keep this registry aligned with
// `get_language_for_ext` so dead rule files do not silently linger.
const PYTHON_GRAPH_QUERY: &str = include_str!("../rules/python.scm");
const JAVA_GRAPH_QUERY: &str = include_str!("../rules/java.scm");
const RUST_GRAPH_QUERY: &str = include_str!("../rules/rust.graph.scm");
const RUST_TAGS_QUERY: &str = include_str!("../rules/rust.tags.scm");
const JAVASCRIPT_GRAPH_QUERY: &str = include_str!("../rules/javascript.scm");
const TYPESCRIPT_GRAPH_QUERY: &str = include_str!("../rules/typescript.graph.scm");
const TYPESCRIPT_TAGS_QUERY: &str = include_str!("../rules/typescript.tags.scm");
const TSX_GRAPH_QUERY: &str = include_str!("../rules/tsx.graph.scm");
const TSX_TAGS_QUERY: &str = TYPESCRIPT_TAGS_QUERY;
const CPP_GRAPH_QUERY: &str = include_str!("../rules/cpp.scm");
const C_GRAPH_QUERY: &str = include_str!("../rules/c.scm");

#[derive(Debug, Clone, Copy)]
enum LanguageRuleset {
    Python,
    Java,
    Rust,
    JavaScript,
    TypeScript,
    Tsx,
    Cpp,
    C,
}

#[derive(Debug, Clone)]
pub struct LanguageConfig {
    pub language: Language,
    pub language_name: &'static str,
    pub graph_query: &'static str,
    pub tags_query: Option<&'static str>,
    ruleset: LanguageRuleset,
}

struct CompiledLanguageRules {
    graph_file: GraphFile,
    tags_query: Option<Query>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TagDefinitionKey {
    name: String,
    start_line: u32,
    start_col: u32,
}

#[derive(Debug, Clone)]
struct TagDefinition {
    key: TagDefinitionKey,
    kind: NodeKind,
    access: Option<AccessKind>,
    canonical_role: CanonicalNodeRole,
    end_line: u32,
    end_col: u32,
}

#[derive(Default)]
struct TagDefinitionIndex {
    by_key: HashMap<TagDefinitionKey, TagDefinition>,
    fallback_index: HashMap<(String, u32), TagDefinitionKey>,
}

fn make_language_config(
    language: Language,
    language_name: &'static str,
    graph_query: &'static str,
    tags_query: Option<&'static str>,
    ruleset: LanguageRuleset,
) -> LanguageConfig {
    LanguageConfig {
        language,
        language_name,
        graph_query,
        tags_query,
        ruleset,
    }
}

impl TagDefinitionIndex {
    fn insert(&mut self, definition: TagDefinition) {
        let key = definition.key.clone();
        match self.by_key.get(&key) {
            Some(existing) if !should_replace_tag_definition(existing, &definition) => {}
            _ => {
                self.fallback_index
                    .insert((key.name.clone(), key.start_line), key.clone());
                self.by_key.insert(key, definition);
            }
        }
    }

    fn take(&mut self, name: &str, start_line: u32, start_col: Option<u32>) -> Option<TagDefinition> {
        if let Some(start_col) = start_col {
            let exact_key = TagDefinitionKey {
                name: name.to_string(),
                start_line,
                start_col,
            };
            if let Some(definition) = self.by_key.remove(&exact_key) {
                self.fallback_index.remove(&(name.to_string(), start_line));
                return Some(definition);
            }
        }

        let fallback_key = self
            .fallback_index
            .remove(&(name.to_string(), start_line))?;
        self.by_key.remove(&fallback_key)
    }

    fn into_remaining(self) -> Vec<TagDefinition> {
        self.by_key.into_values().collect()
    }
}

impl LanguageConfig {
    fn compiled_rules(&self) -> Result<&'static CompiledLanguageRules> {
        self.ruleset.compiled_rules(self.language.clone())
    }
}

impl LanguageRuleset {
    fn compiled_rules(&self, language: Language) -> Result<&'static CompiledLanguageRules> {
        match self {
            LanguageRuleset::Python => compiled_rules_cache(language, PYTHON_GRAPH_QUERY, None, &PYTHON_RULES),
            LanguageRuleset::Java => compiled_rules_cache(language, JAVA_GRAPH_QUERY, None, &JAVA_RULES),
            LanguageRuleset::Rust => compiled_rules_cache(
                language,
                RUST_GRAPH_QUERY,
                Some(RUST_TAGS_QUERY),
                &RUST_RULES,
            ),
            LanguageRuleset::JavaScript => compiled_rules_cache(
                language,
                JAVASCRIPT_GRAPH_QUERY,
                None,
                &JAVASCRIPT_RULES,
            ),
            LanguageRuleset::TypeScript => compiled_rules_cache(
                language,
                TYPESCRIPT_GRAPH_QUERY,
                Some(TYPESCRIPT_TAGS_QUERY),
                &TYPESCRIPT_RULES,
            ),
            LanguageRuleset::Tsx => compiled_rules_cache(
                language,
                TSX_GRAPH_QUERY,
                Some(TSX_TAGS_QUERY),
                &TSX_RULES,
            ),
            LanguageRuleset::Cpp => compiled_rules_cache(language, CPP_GRAPH_QUERY, None, &CPP_RULES),
            LanguageRuleset::C => compiled_rules_cache(language, C_GRAPH_QUERY, None, &C_RULES),
        }
    }
}

fn compiled_rules_cache(
    language: Language,
    graph_query: &'static str,
    tags_query: Option<&'static str>,
    cache: &'static OnceLock<Result<CompiledLanguageRules, String>>,
) -> Result<&'static CompiledLanguageRules> {
    let compiled = cache.get_or_init(|| {
        let graph_file = GraphFile::from_str(language.clone(), graph_query)
            .map_err(|e| format!("Graph DSL error: {:?}", e))?;
        let tags_query = tags_query
            .filter(|query| !query.trim().is_empty())
            .map(|query| Query::new(&language, query).map_err(|e| format!("Tag query error: {:?}", e)))
            .transpose()?;
        Ok::<CompiledLanguageRules, String>(CompiledLanguageRules {
            graph_file,
            tags_query,
        })
    });

    compiled.as_ref().map_err(|message| anyhow!(message.clone()))
}

static PYTHON_RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();
static JAVA_RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();
static RUST_RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();
static JAVASCRIPT_RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();
static TYPESCRIPT_RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();
static TSX_RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();
static CPP_RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();
static C_RULES: OnceLock<Result<CompiledLanguageRules, String>> = OnceLock::new();

fn tag_definition_priority(definition: &TagDefinition) -> (u8, u8, u8) {
    let role_priority = canonical_role_priority(definition.canonical_role);
    let kind_priority = match definition.kind {
        NodeKind::METHOD => 7,
        NodeKind::FUNCTION => 6,
        NodeKind::FIELD => 5,
        NodeKind::STRUCT => 4,
        NodeKind::CLASS => 4,
        NodeKind::INTERFACE => 4,
        NodeKind::ENUM => 4,
        NodeKind::UNION => 4,
        NodeKind::TYPEDEF => 4,
        _ => 1,
    };
    let access_priority = u8::from(definition.access.is_some());
    (role_priority, kind_priority, access_priority)
}

fn should_replace_tag_definition(existing: &TagDefinition, candidate: &TagDefinition) -> bool {
    tag_definition_priority(candidate) > tag_definition_priority(existing)
}

fn tag_definition_kind(kind: &str) -> Option<NodeKind> {
    match kind {
        "class" => Some(NodeKind::CLASS),
        "struct" => Some(NodeKind::STRUCT),
        "interface" => Some(NodeKind::INTERFACE),
        "enum" => Some(NodeKind::ENUM),
        "typedef" => Some(NodeKind::TYPEDEF),
        "union" => Some(NodeKind::UNION),
        "function" => Some(NodeKind::FUNCTION),
        "method" => Some(NodeKind::METHOD),
        "field" => Some(NodeKind::FIELD),
        "variable" => Some(NodeKind::VARIABLE),
        _ => None,
    }
}

fn parse_access_capture_text(text: &str) -> Option<AccessKind> {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("pub") {
        return Some(AccessKind::Public);
    }
    access_kind_from_graph_access(&lower).or_else(|| classify_keyword_access(trimmed))
}

fn extract_tag_definitions(
    compiled_rules: &CompiledLanguageRules,
    tree: &Tree,
    source: &str,
) -> Result<TagDefinitionIndex> {
    let Some(query) = compiled_rules.tags_query.as_ref() else {
        return Ok(TagDefinitionIndex::default());
    };

    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut index = TagDefinitionIndex::default();
    let source_bytes = source.as_bytes();

    let mut matches = cursor.matches(&query, tree.root_node(), source_bytes);
    while {
        matches.advance();
        matches.get().is_some()
    } {
        let Some(query_match) = matches.get() else {
            continue;
        };
        let mut definition: Option<TagDefinition> = None;
        let mut access = None;
        let mut canonical_role = CanonicalNodeRole::Unspecified;

        for capture in query_match.captures {
            let capture_name = capture_names
                .get(capture.index as usize)
                .map(|name| *name)
                .unwrap_or_default();
            let capture_node = capture.node;
            if let Some(kind_name) = capture_name.strip_prefix("definition.") {
                let Some(kind) = tag_definition_kind(kind_name) else {
                    continue;
                };
                let name = capture_node
                    .utf8_text(source_bytes)
                    .map(str::trim)
                    .unwrap_or_default()
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let start = capture_node.start_position();
                let end = capture_node.end_position();
                definition = Some(TagDefinition {
                    key: TagDefinitionKey {
                        name,
                        start_line: start.row as u32 + 1,
                        start_col: start.column as u32 + 1,
                    },
                    kind,
                    access: None,
                    canonical_role: CanonicalNodeRole::Unspecified,
                    end_line: end.row as u32 + 1,
                    end_col: end.column as u32 + 1,
                });
            } else if capture_name == "access" {
                let text = capture_node.utf8_text(source_bytes).unwrap_or_default();
                access = parse_access_capture_text(text);
            } else if capture_name == "canonical.impl_anchor" {
                canonical_role = CanonicalNodeRole::ImplAnchor;
            }
        }

        if let Some(mut definition) = definition {
            definition.access = access;
            definition.canonical_role = canonical_role;
            index.insert(definition);
        }
    }

    Ok(index)
}

fn infer_header_language_config(
    compilation_info: Option<&compilation_database::CompilationInfo>,
) -> LanguageConfig {
    let use_cpp = compilation_info
        .and_then(|info| info.standard)
        .map(|standard| {
            matches!(
                standard,
                compilation_database::CxxStandard::Cxx98
                    | compilation_database::CxxStandard::Cxx03
                    | compilation_database::CxxStandard::Cxx11
                    | compilation_database::CxxStandard::Cxx14
                    | compilation_database::CxxStandard::Cxx17
                    | compilation_database::CxxStandard::Cxx20
                    | compilation_database::CxxStandard::Cxx23
            )
        })
        .unwrap_or(false);

    if use_cpp {
        make_language_config(
            tree_sitter_cpp::LANGUAGE.into(),
            "cpp",
            CPP_GRAPH_QUERY,
            None,
            LanguageRuleset::Cpp,
        )
    } else {
        make_language_config(
            tree_sitter_c::LANGUAGE.into(),
            "c",
            C_GRAPH_QUERY,
            None,
            LanguageRuleset::C,
        )
    }
}

fn get_language_config_for_path(
    path: &Path,
    compilation_info: Option<&compilation_database::CompilationInfo>,
) -> Option<LanguageConfig> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    if ext.trim().trim_start_matches('.').eq_ignore_ascii_case("h") {
        return Some(infer_header_language_config(compilation_info));
    }
    get_language_for_ext(ext)
}

#[derive(Debug, Clone, Copy)]
pub struct IncrementalIndexingConfig {
    pub file_batch_size: usize,
    pub node_batch_size: usize,
    pub edge_batch_size: usize,
    pub occurrence_batch_size: usize,
    pub error_batch_size: usize,
}

impl Default for IncrementalIndexingConfig {
    fn default() -> Self {
        Self {
            file_batch_size: 16,
            node_batch_size: 50_000,
            edge_batch_size: 50_000,
            occurrence_batch_size: 50_000,
            error_batch_size: 1_000,
        }
    }
}

pub struct IndexResult {
    pub files: Vec<codestory_storage::FileInfo>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub occurrences: Vec<Occurrence>,
    pub component_access: Vec<(NodeId, AccessKind)>,
    pub callable_projection_states: Vec<CallableProjectionState>,
    pub impl_anchor_node_ids: Vec<NodeId>,
}

const FILE_STRUCTURAL_SYMBOL_KEY: &str = "__file_structural__";

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProjectionUpdateMode {
    InsertFresh,
    NoChanges,
    Delta { changed_callers: Vec<NodeId> },
    FullReplace,
}

pub enum IndexingEvent {
    Progress(u64),
    Error(String),
    Finished,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IncrementalIndexingStats {
    pub parse_index_ms: u64,
    pub projection_flush_ms: u64,
    pub edge_resolution_ms: u64,
    pub error_flush_ms: u64,
    pub cleanup_ms: u64,
    pub unresolved_calls_start: usize,
    pub unresolved_imports_start: usize,
    pub resolved_calls: usize,
    pub resolved_imports: usize,
    pub unresolved_calls_end: usize,
    pub unresolved_imports_end: usize,
    pub resolution_ran: bool,
    pub resolution_unresolved_counts_ms: u64,
    pub resolution_calls_ms: u64,
    pub resolution_imports_ms: u64,
    pub resolution_cleanup_ms: u64,
    pub resolved_calls_same_file: usize,
    pub resolved_calls_same_module: usize,
    pub resolved_calls_global_unique: usize,
    pub resolved_calls_semantic: usize,
    pub resolved_imports_same_file: usize,
    pub resolved_imports_same_module: usize,
    pub resolved_imports_global_unique: usize,
    pub resolved_imports_fuzzy: usize,
    pub resolved_imports_semantic: usize,
}

pub struct WorkspaceIndexer {
    root: PathBuf,
    compilation_db: Option<compilation_database::CompilationDatabase>,
    compilation_db_warning: Option<String>,
    batch_config: IncrementalIndexingConfig,
}

impl WorkspaceIndexer {
    pub fn new(root: PathBuf) -> Self {
        let (compilation_db, compilation_db_warning) = if let Some(path) =
            compilation_database::CompilationDatabase::find_in_directory(&root)
        {
            match compilation_database::CompilationDatabase::load(&path) {
                Ok(db) => (Some(db), None),
                Err(err) => (
                    None,
                    Some(format!(
                        "Failed to load compile_commands.json at {}: {}. Continuing without compilation metadata.",
                        path.display(),
                        err
                    )),
                ),
            }
        } else {
            (None, None)
        };
        Self {
            root,
            compilation_db,
            compilation_db_warning,
            batch_config: IncrementalIndexingConfig::default(),
        }
    }

    pub fn with_batch_config(mut self, batch_config: IncrementalIndexingConfig) -> Self {
        self.batch_config = batch_config;
        self
    }

    /// Returns the workspace root path
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn run_incremental(
        &self,
        storage: &mut Storage,
        refresh_info: &codestory_project::RefreshInfo,
        event_bus: &EventBus,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<IncrementalIndexingStats> {
        event_bus.publish(Event::IndexingStarted {
            file_count: refresh_info.files_to_index.len(),
        });
        if let Some(message) = &self.compilation_db_warning {
            event_bus.publish(Event::ShowWarning {
                message: message.clone(),
            });
        }
        let mut stats = IncrementalIndexingStats::default();
        let total_files = refresh_info.files_to_index.len();
        let processed_count = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let root = self.root.clone();
        let existing_projection_ids =
            Self::existing_projection_ids(storage, &root, &refresh_info.files_to_index);
        let mut replaced_projection_ids = HashSet::new();

        let symbol_table = Arc::new(SymbolTable::new());
        Self::seed_symbol_table(storage, &symbol_table);

        // Clone for parallel closure
        let cancelled_clone = cancelled.clone();
        if Self::is_cancelled(cancel_token) {
            return Ok(stats);
        }

        // 1. Parallel Indexing (chunked and flushed)
        let mut batched_storage = IntermediateStorage::default();
        let mut all_errors = Vec::new();
        let mut had_edges = false;
        let file_batch_size = self.batch_config.file_batch_size.max(1);
        let batch_config = self.batch_config;

        for file_chunk in refresh_info.files_to_index.chunks(file_batch_size) {
            let chunk_started = Instant::now();
            let chunk_results: Vec<IntermediateStorage> = file_chunk
                .par_iter()
                .map(|path| {
                    // Check cancellation
                    if let Some(token) = cancel_token
                        && token.is_cancelled()
                    {
                        cancelled_clone.store(true, Ordering::Relaxed);
                        return IntermediateStorage::default();
                    }

                    let local_storage = self.index_path(path, &root, &symbol_table);

                    let current = processed_count.fetch_add(1, Ordering::Relaxed) + 1;
                    event_bus.publish(Event::IndexingProgress {
                        current,
                        total: total_files,
                    });

                    local_storage
                })
                .collect();
            stats.parse_index_ms = stats
                .parse_index_ms
                .saturating_add(duration_ms_u64(chunk_started.elapsed()));

            for mut local_storage in chunk_results {
                all_errors.append(&mut local_storage.errors);
                if let Some(file_info) = local_storage.files.first()
                    && existing_projection_ids.contains_key(&file_info.path)
                    && replaced_projection_ids.insert(file_info.id)
                {
                    let existing_states = storage
                        .get_callable_projection_states_for_file(file_info.id)
                        .map_err(|e| anyhow!("Storage state lookup error: {:?}", e))?;
                    let cleanup_started = Instant::now();
                    let update_mode = classify_projection_update(
                        &existing_states,
                        &local_storage.callable_projection_states,
                    );
                    match update_mode {
                        ProjectionUpdateMode::InsertFresh => {}
                        ProjectionUpdateMode::NoChanges => {}
                        ProjectionUpdateMode::Delta { changed_callers } => {
                            storage
                                .delete_projection_for_callers(file_info.id, &changed_callers)
                                .map_err(|e| anyhow!("Storage delta cleanup error: {:?}", e))?;
                        }
                        ProjectionUpdateMode::FullReplace => {
                            storage
                                .delete_file_projection(file_info.id)
                                .map_err(|e| anyhow!("Storage cleanup error: {:?}", e))?;
                        }
                    }
                    stats.cleanup_ms = stats
                        .cleanup_ms
                        .saturating_add(duration_ms_u64(cleanup_started.elapsed()));
                }
                batched_storage.merge(local_storage);
                reconcile_rust_impl_anchors(storage, &mut batched_storage)?;

                let should_flush = !batched_storage.files.is_empty()
                    || !batched_storage.nodes.is_empty()
                    || !batched_storage.edges.is_empty()
                    || !batched_storage.occurrences.is_empty();
                if should_flush
                    && (batched_storage.nodes.len() >= batch_config.node_batch_size
                        || batched_storage.edges.len() >= batch_config.edge_batch_size
                        || batched_storage.occurrences.len() >= batch_config.occurrence_batch_size)
                {
                    let flush_started = Instant::now();
                    Self::flush_projection_batch(storage, &mut batched_storage, &mut had_edges)?;
                    stats.projection_flush_ms = stats
                        .projection_flush_ms
                        .saturating_add(duration_ms_u64(flush_started.elapsed()));
                }

                if all_errors.len() >= batch_config.error_batch_size {
                    let error_flush_started = Instant::now();
                    Self::flush_errors(storage, &mut all_errors, batch_config.error_batch_size)?;
                    stats.error_flush_ms = stats
                        .error_flush_ms
                        .saturating_add(duration_ms_u64(error_flush_started.elapsed()));
                }
            }

            if cancelled.load(Ordering::Relaxed) {
                event_bus.publish(Event::IndexingComplete { duration_ms: 0 });
                return Ok(stats);
            }
        }

        // Check if cancelled during indexing
        if cancelled.load(Ordering::Relaxed) {
            event_bus.publish(Event::IndexingComplete { duration_ms: 0 });
            return Ok(stats);
        }

        let flush_started = Instant::now();
        Self::flush_projection_batch(storage, &mut batched_storage, &mut had_edges)?;
        stats.projection_flush_ms = stats
            .projection_flush_ms
            .saturating_add(duration_ms_u64(flush_started.elapsed()));

        // 3.5 Resolve call/import edges post-pass
        if had_edges {
            let resolver = resolution::ResolutionPass::new();
            let resolution_scope_file_ids =
                Self::collect_touched_file_ids(&root, &refresh_info.files_to_index);
            let resolution_scope =
                (!resolution_scope_file_ids.is_empty()).then_some(&resolution_scope_file_ids);
            let (unresolved_calls_start, unresolved_imports_start) =
                resolver.unresolved_counts_with_scope(storage, resolution_scope)?;
            let unresolved_overrides_start = storage
                .get_edges()?
                .into_iter()
                .filter(|edge| {
                    edge.kind == EdgeKind::OVERRIDE
                        && edge.resolved_target.is_none()
                        && resolution_scope.is_none_or(|scope| {
                            edge.file_node_id.is_some_and(|file_id| scope.contains(&file_id.0))
                        })
                })
                .count();
            stats.unresolved_calls_start = unresolved_calls_start;
            stats.unresolved_imports_start = unresolved_imports_start;
            let scope_suffix = resolution_scope
                .map(|scope| format!(" (scoped to {} touched files)", scope.len()))
                .unwrap_or_default();
            event_bus.publish(Event::StatusUpdate {
                message: format!(
                    "Resolution pass starting with {unresolved_calls_start} unresolved CALL edges, {unresolved_imports_start} unresolved IMPORT edges, and {unresolved_overrides_start} unresolved OVERRIDE edges{scope_suffix}."
                ),
            });
            let resolution_started = Instant::now();
            let resolution_stats = resolver
                .run_with_scope(storage, resolution_scope)
                .map_err(|e| anyhow!("Resolution error: {:?}", e))?;
            stats.edge_resolution_ms = stats
                .edge_resolution_ms
                .saturating_add(duration_ms_u64(resolution_started.elapsed()));
            stats.resolution_ran = true;
            stats.resolved_calls = resolution_stats.resolved_calls;
            stats.resolved_imports = resolution_stats.resolved_imports;
            stats.unresolved_calls_end = resolution_stats.unresolved_calls;
            stats.unresolved_imports_end = resolution_stats.unresolved_imports;
            stats.resolution_unresolved_counts_ms = resolution_stats
                .telemetry
                .unresolved_count_start_ms
                .saturating_add(resolution_stats.telemetry.unresolved_count_end_ms);
            stats.resolution_calls_ms = resolution_stats
                .telemetry
                .call_prepare_ms
                .saturating_add(resolution_stats.telemetry.call_unresolved_load_ms)
                .saturating_add(resolution_stats.telemetry.call_candidate_index_ms)
                .saturating_add(resolution_stats.telemetry.call_compute_ms)
                .saturating_add(resolution_stats.telemetry.call_apply_ms);
            stats.resolution_imports_ms = resolution_stats
                .telemetry
                .import_prepare_ms
                .saturating_add(resolution_stats.telemetry.import_unresolved_load_ms)
                .saturating_add(resolution_stats.telemetry.import_candidate_index_ms)
                .saturating_add(resolution_stats.telemetry.import_compute_ms)
                .saturating_add(resolution_stats.telemetry.import_apply_ms);
            stats.resolution_cleanup_ms = resolution_stats
                .telemetry
                .scope_prepare_ms
                .saturating_add(resolution_stats.telemetry.call_cleanup_ms);
            stats.resolved_calls_same_file = resolution_stats.strategy_counters.call_same_file;
            stats.resolved_calls_same_module =
                resolution_stats.strategy_counters.call_same_module;
            stats.resolved_calls_global_unique =
                resolution_stats.strategy_counters.call_global_unique;
            stats.resolved_calls_semantic =
                resolution_stats.strategy_counters.call_semantic_fallback;
            stats.resolved_imports_same_file =
                resolution_stats.strategy_counters.import_same_file;
            stats.resolved_imports_same_module =
                resolution_stats.strategy_counters.import_same_module;
            stats.resolved_imports_global_unique =
                resolution_stats.strategy_counters.import_global_unique;
            stats.resolved_imports_fuzzy = resolution_stats.strategy_counters.import_fuzzy;
            stats.resolved_imports_semantic =
                resolution_stats.strategy_counters.import_semantic_fallback;
        }

        // Write errors
        while !all_errors.is_empty() {
            let error_flush_started = Instant::now();
            Self::flush_errors(storage, &mut all_errors, batch_config.error_batch_size)?;
            stats.error_flush_ms = stats
                .error_flush_ms
                .saturating_add(duration_ms_u64(error_flush_started.elapsed()));
        }

        // 4. Cleanup removed files
        if !refresh_info.files_to_remove.is_empty() {
            let cleanup_started = Instant::now();
            storage
                .delete_files_batch(&refresh_info.files_to_remove)
                .map_err(|e| anyhow!("Storage cleanup error: {:?}", e))?;
            stats.cleanup_ms = stats
                .cleanup_ms
                .saturating_add(duration_ms_u64(cleanup_started.elapsed()));
        }

        event_bus.publish(Event::IndexingComplete { duration_ms: 0 });
        Ok(stats)
    }

    fn is_cancelled(cancel_token: Option<&CancellationToken>) -> bool {
        cancel_token
            .map(CancellationToken::is_cancelled)
            .unwrap_or(false)
    }

    fn seed_symbol_table(storage: &Storage, symbol_table: &SymbolTable) {
        if let Ok(nodes) = storage.get_nodes() {
            for node in nodes {
                symbol_table.insert(node.id.0, node.kind);
            }
        }
    }

    fn collect_touched_file_ids(root: &Path, files_to_index: &[PathBuf]) -> HashSet<i64> {
        let mut file_ids = HashSet::new();
        for path in files_to_index {
            let full_path = Self::normalize_index_path(root, path);
            file_ids.insert(Self::canonical_file_node_id_for_path(&full_path));
            if let Ok(canonical) = full_path.canonicalize() {
                file_ids.insert(Self::canonical_file_node_id_for_path(&canonical));
            }
        }
        file_ids
    }

    fn normalize_index_path(root: &Path, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        }
    }

    fn existing_projection_ids(
        storage: &Storage,
        root: &Path,
        files_to_index: &[PathBuf],
    ) -> HashMap<PathBuf, i64> {
        let mut out = HashMap::new();
        for path in files_to_index {
            let full_path = Self::normalize_index_path(root, path);
            if let Ok(Some(file_info)) = storage.get_file_by_path(&full_path) {
                out.insert(file_info.path, file_info.id);
            }
        }
        out
    }

    fn canonical_file_node_id_for_path(path: &Path) -> i64 {
        let file_name = path.to_string_lossy();
        let canonical_id = format!("{file_name}:{file_name}:1");
        generate_id(&canonical_id)
    }

    fn flush_errors(
        storage: &mut Storage,
        errors: &mut Vec<codestory_core::ErrorInfo>,
        error_batch_size: usize,
    ) -> Result<()> {
        if errors.is_empty() {
            return Ok(());
        }

        let take_count = errors.len().min(error_batch_size.max(1));
        let drain = errors.drain(..take_count).collect::<Vec<_>>();
        for error in drain {
            storage
                .insert_error(&error)
                .map_err(|e| anyhow!("Storage error: {:?}", e))?;
        }

        Ok(())
    }

    fn flush_projection_batch(
        storage: &mut Storage,
        batched_storage: &mut IntermediateStorage,
        had_edges: &mut bool,
    ) -> Result<()> {
        reconcile_rust_impl_anchors(storage, batched_storage)?;
        if !batched_storage.files.is_empty() {
            storage
                .insert_files_batch(&batched_storage.files)
                .map_err(|e| anyhow!("Storage error: {:?}", e))?;
        }
        if !batched_storage.nodes.is_empty() {
            storage
                .insert_nodes_batch(&batched_storage.nodes)
                .map_err(|e| anyhow!("Storage error: {:?}", e))?;
        }
        if !batched_storage.edges.is_empty() {
            storage
                .insert_edges_batch(&batched_storage.edges)
                .map_err(|e| anyhow!("Storage error: {:?}", e))?;
            *had_edges = true;
        }
        if !batched_storage.occurrences.is_empty() {
            storage
                .insert_occurrences_batch(&batched_storage.occurrences)
                .map_err(|e| anyhow!("Storage error: {:?}", e))?;
        }
        if !batched_storage.component_access.is_empty() {
            storage
                .insert_component_access_batch(&batched_storage.component_access)
                .map_err(|e| anyhow!("Storage error: {:?}", e))?;
        }
        if !batched_storage.callable_projection_states.is_empty() {
            storage
                .upsert_callable_projection_states(&batched_storage.callable_projection_states)
                .map_err(|e| anyhow!("Storage error: {:?}", e))?;
        }

        batched_storage.clear();
        Ok(())
    }

    fn index_path(
        &self,
        path: &PathBuf,
        root: &Path,
        symbol_table: &Arc<SymbolTable>,
    ) -> IntermediateStorage {
        let full_path = Self::normalize_index_path(root, path);

        let mut local_storage = IntermediateStorage::default();
        let compilation_info = self
            .compilation_db
            .as_ref()
            .and_then(|db| db.get_parsed_info(&full_path));
        let Some(language_config) =
            get_language_config_for_path(&full_path, compilation_info.as_ref())
        else {
            return local_storage;
        };

        match std::fs::read(&full_path) {
            Ok(bytes) => {
                // Some third-party/vendor sources contain legacy bytes but are still parseable enough
                // for indexing once we decode them lossily.
                let source = String::from_utf8_lossy(&bytes).into_owned();
                match index_file(
                    &full_path,
                    &source,
                    &language_config,
                    compilation_info,
                    Some(Arc::clone(symbol_table)),
                ) {
                    Ok(index_result) => {
                        local_storage.files = index_result.files;
                        local_storage.nodes = index_result.nodes;
                        local_storage.edges = index_result.edges;
                        local_storage.occurrences = index_result.occurrences;
                        local_storage.component_access = index_result.component_access;
                        local_storage.callable_projection_states =
                            index_result.callable_projection_states;
                        local_storage.impl_anchor_node_ids = index_result.impl_anchor_node_ids;
                    }
                    Err(e) => {
                        local_storage.add_error(codestory_core::ErrorInfo {
                            message: format!(
                                "Failed to index {:?}: {}",
                                full_path.strip_prefix(root).unwrap_or(&full_path),
                                e
                            ),
                            file_id: None,
                            line: None,
                            column: None,
                            is_fatal: false,
                            index_step: codestory_core::IndexStep::Indexing,
                        });
                    }
                }
            }
            Err(e) => {
                local_storage.add_error(codestory_core::ErrorInfo {
                    message: format!("Failed to read {:?}: {}", path, e),
                    file_id: None,
                    line: None,
                    column: None,
                    is_fatal: true,
                    index_step: codestory_core::IndexStep::Collection,
                });
            }
        }

        local_storage
    }
}

fn duration_ms_u64(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

fn file_node_from_source(path: &Path, source: &str) -> (Node, String, NodeId) {
    let file_name = path.to_string_lossy().to_string();
    let file_id = NodeId(WorkspaceIndexer::canonical_file_node_id_for_path(path));
    let line_count = source.lines().count() as u32;
    let file_end_line = if line_count == 0 { 1 } else { line_count };

    let file_node = Node {
        id: file_id,
        kind: NodeKind::FILE,
        serialized_name: file_name.clone(),
        start_line: Some(1),
        start_col: Some(1),
        end_line: Some(file_end_line),
        ..Default::default()
    };

    (file_node, file_name, file_id)
}

fn node_kind_from_graph_kind(kind_str: &str) -> NodeKind {
    match kind_str {
        "MODULE" => NodeKind::MODULE,
        "NAMESPACE" => NodeKind::NAMESPACE,
        "PACKAGE" => NodeKind::PACKAGE,
        "FILE" => NodeKind::FILE,
        "STRUCT" => NodeKind::STRUCT,
        "CLASS" => NodeKind::CLASS,
        "INTERFACE" => NodeKind::INTERFACE,
        "ANNOTATION" => NodeKind::ANNOTATION,
        "UNION" => NodeKind::UNION,
        "ENUM" => NodeKind::ENUM,
        "TYPEDEF" => NodeKind::TYPEDEF,
        "TYPE_PARAMETER" => NodeKind::TYPE_PARAMETER,
        "BUILTIN_TYPE" => NodeKind::BUILTIN_TYPE,
        "FUNCTION" => NodeKind::FUNCTION,
        "METHOD" => NodeKind::METHOD,
        "MACRO" => NodeKind::MACRO,
        "GLOBAL_VARIABLE" => NodeKind::GLOBAL_VARIABLE,
        "FIELD" => NodeKind::FIELD,
        "VARIABLE" => NodeKind::VARIABLE,
        "CONSTANT" => NodeKind::CONSTANT,
        "ENUM_CONSTANT" => NodeKind::ENUM_CONSTANT,
        _ => NodeKind::UNKNOWN,
    }
}

fn access_kind_from_graph_access(value: &str) -> Option<AccessKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "public" => Some(AccessKind::Public),
        "protected" => Some(AccessKind::Protected),
        "private" => Some(AccessKind::Private),
        "default" | "package" | "package_private" => Some(AccessKind::Default),
        _ => None,
    }
}

fn is_python_constant_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
        && trimmed.chars().any(|ch| ch.is_ascii_uppercase())
}

fn source_line(source: &str, line: u32) -> Option<&str> {
    if line == 0 {
        return None;
    }
    source.lines().nth((line - 1) as usize)
}

fn classify_keyword_access(text: &str) -> Option<AccessKind> {
    let trimmed = text.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "private" | "private:" | "private =" | "private{" | "private("
    ) || lower.starts_with("private ")
        || lower.starts_with("private\t")
    {
        return Some(AccessKind::Private);
    }
    if matches!(
        lower.as_str(),
        "protected" | "protected:" | "protected =" | "protected{" | "protected("
    ) || lower.starts_with("protected ")
        || lower.starts_with("protected\t")
    {
        return Some(AccessKind::Protected);
    }
    if matches!(
        lower.as_str(),
        "public" | "public:" | "public =" | "public{" | "public("
    ) || lower.starts_with("public ")
        || lower.starts_with("public\t")
    {
        return Some(AccessKind::Public);
    }
    None
}

fn classify_rust_visibility(text: &str) -> Option<AccessKind> {
    let trimmed = text.trim_start();
    if trimmed.starts_with("pub(")
        || trimmed.starts_with("pub ")
        || trimmed.starts_with("pub\t")
        || trimmed == "pub"
    {
        return Some(AccessKind::Public);
    }
    None
}

fn point_for_line_start(line: u32) -> Point {
    Point {
        row: line.saturating_sub(1) as usize,
        column: 0,
    }
}

fn infer_cpp_access_from_tree(tree: &Tree, source: &str, start_line: u32) -> Option<AccessKind> {
    let root = tree.root_node();
    let point = point_for_line_start(start_line);
    let mut node = root.named_descendant_for_point_range(point, point)?;

    loop {
        if node.kind() == "field_declaration_list" {
            let container_kind = node.parent().map(|parent| parent.kind()).unwrap_or_default();
            let mut current = if container_kind == "struct_specifier" {
                AccessKind::Public
            } else {
                AccessKind::Private
            };
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "access_specifier" {
                    let text = child.utf8_text(source.as_bytes()).unwrap_or_default();
                    if let Some(access) = classify_keyword_access(text) {
                        current = access;
                    }
                    continue;
                }

                let start_row = child.start_position().row as u32 + 1;
                let end_row = child.end_position().row as u32 + 1;
                if start_line >= start_row && start_line <= end_row {
                    return Some(current);
                }
            }
            return Some(current);
        }

        let Some(parent) = node.parent() else {
            break;
        };
        node = parent;
    }

    None
}

#[derive(Debug, Clone)]
struct ManualEdgeSpec {
    source_name: String,
    target_name: String,
    kind: EdgeKind,
    line: Option<u32>,
}

fn node_source_text(node: TsNode<'_>, source: &str) -> Option<String> {
    source.get(node.byte_range()).map(ToString::to_string)
}

fn split_top_level_type_arguments(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    let inner = if let (Some(start), Some(end)) = (trimmed.find('<'), trimmed.rfind('>')) {
        if end > start {
            &trimmed[start + 1..end]
        } else {
            trimmed
        }
    } else {
        trimmed
            .strip_prefix('<')
            .and_then(|value| value.strip_suffix('>'))
            .unwrap_or(trimmed)
    };
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut angle_depth = 0i32;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;

    for ch in inner.chars() {
        match ch {
            '<' => {
                angle_depth += 1;
                current.push(ch);
            }
            '>' => {
                angle_depth = (angle_depth - 1).max(0);
                current.push(ch);
            }
            '(' => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' => {
                paren_depth = (paren_depth - 1).max(0);
                current.push(ch);
            }
            '[' => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' => {
                bracket_depth = (bracket_depth - 1).max(0);
                current.push(ch);
            }
            ',' if angle_depth == 0 && paren_depth == 0 && bracket_depth == 0 => {
                let part = current.trim();
                if !part.is_empty() {
                    parts.push(part.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let tail = current.trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    parts
}

fn walk_tree_nodes<F>(node: TsNode<'_>, visit: &mut F)
where
    F: FnMut(TsNode<'_>),
{
    visit(node);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_tree_nodes(child, visit);
    }
}

fn is_rust_local_symbol_import_path(name: &str) -> bool {
    let Some(last_segment) = name.rsplit("::").next() else {
        return false;
    };
    (name.starts_with("crate::") || name.starts_with("self::") || name.starts_with("super::"))
        && last_segment
            .chars()
            .next()
            .map(|ch| ch.is_ascii_uppercase())
            .unwrap_or(false)
}

fn collect_rust_generic_type_argument_edges(tree: &Tree, source: &str) -> Vec<ManualEdgeSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "call_expression" {
            return;
        }
        let Some(function_node) = node.child_by_field_name("function") else {
            return;
        };
        if function_node.kind() != "generic_function" {
            return;
        }
        let Some(callee_node) = function_node.child_by_field_name("function") else {
            return;
        };
        let Some(callee_name) = node_source_text(callee_node, source) else {
            return;
        };
        let Some(type_arguments_node) = function_node.child_by_field_name("type_arguments") else {
            return;
        };
        let line = Some(node.start_position().row as u32 + 1);
        edges.push(ManualEdgeSpec {
            source_name: callee_name.clone(),
            target_name: callee_name.clone(),
            kind: EdgeKind::CALL,
            line,
        });

        let Some(raw_arguments) = node_source_text(type_arguments_node, source) else {
            return;
        };
        for type_name in split_top_level_type_arguments(&raw_arguments) {
            edges.push(ManualEdgeSpec {
                source_name: callee_name.clone(),
                target_name: type_name,
                kind: EdgeKind::TYPE_ARGUMENT,
                line,
            });
        }
    });
    edges
}

fn collect_cpp_template_type_argument_edges(tree: &Tree, source: &str) -> Vec<ManualEdgeSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "template_type" {
            return;
        }
        let Some(template_name) = cpp_named_type_text(node.child_by_field_name("name"), source) else {
            return;
        };
        let Some(arguments) = node.child_by_field_name("arguments") else {
            return;
        };
        let line = Some(node.start_position().row as u32 + 1);
        let mut cursor = arguments.walk();
        for argument in arguments.named_children(&mut cursor) {
            let Some(argument_name) = cpp_named_type_text(Some(argument), source) else {
                continue;
            };
            edges.push(ManualEdgeSpec {
                source_name: template_name.clone(),
                target_name: argument_name,
                kind: EdgeKind::TYPE_ARGUMENT,
                line,
            });
        }
    });
    edges
}

fn cpp_named_type_text(node: Option<TsNode<'_>>, source: &str) -> Option<String> {
    let node = node?;
    match node.kind() {
        "template_type" => cpp_named_type_text(node.child_by_field_name("name"), source),
        "type_descriptor" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(text) = cpp_named_type_text(Some(child), source) {
                    return Some(text);
                }
            }
            node_source_text(node, source).map(|text| {
                text.trim()
                    .trim_start_matches("typename ")
                    .trim_start_matches("class ")
                    .trim()
                    .to_string()
            })
        }
        "type_identifier" | "qualified_identifier" | "primitive_type" | "identifier"
        | "namespace_identifier" | "field_identifier" => {
            node_source_text(node, source).map(|text| {
                text.trim()
                    .trim_start_matches("typename ")
                    .trim_start_matches("class ")
                    .trim()
                    .to_string()
            })
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(text) = cpp_named_type_text(Some(child), source) {
                    return Some(text);
                }
            }
            None
        }
    }
}

fn collect_tsx_jsx_usage_edges(tree: &Tree, source: &str) -> Vec<ManualEdgeSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        let source_name = match node.kind() {
            "function_declaration" | "method_definition" => node
                .child_by_field_name("name")
                .and_then(|name| node_source_text(name, source))
                .map(|name| name.trim().to_string()),
            _ => None,
        };
        let Some(source_name) = source_name else {
            return;
        };
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        collect_tsx_return_usage_edges(Some(body), &source_name, source, &mut edges);
    });
    edges
}

fn collect_tsx_return_usage_edges(
    node: Option<TsNode<'_>>,
    source_name: &str,
    source: &str,
    edges: &mut Vec<ManualEdgeSpec>,
) {
    let Some(node) = node else {
        return;
    };
    if node.kind() == "return_statement" {
        let line = Some(node.start_position().row as u32 + 1);
        collect_tsx_jsx_targets(Some(node), source_name, source, line, edges);
        return;
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_tsx_return_usage_edges(Some(child), source_name, source, edges);
    }
}

fn collect_tsx_jsx_targets(
    node: Option<TsNode<'_>>,
    source_name: &str,
    source: &str,
    line: Option<u32>,
    edges: &mut Vec<ManualEdgeSpec>,
) {
    let Some(node) = node else {
        return;
    };
    match node.kind() {
        "jsx_self_closing_element" => {
            if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|name| node_source_text(name, source))
            {
                edges.push(ManualEdgeSpec {
                    source_name: source_name.to_string(),
                    target_name: name.trim().to_string(),
                    kind: EdgeKind::USAGE,
                    line,
                });
            }
        }
        "jsx_opening_element" => {
            if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|name| node_source_text(name, source))
            {
                edges.push(ManualEdgeSpec {
                    source_name: source_name.to_string(),
                    target_name: name.trim().to_string(),
                    kind: EdgeKind::USAGE,
                    line,
                });
            }
        }
        "jsx_attribute" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() != "property_identifier" {
                    continue;
                }
                if let Some(name) = node_source_text(child, source) {
                    edges.push(ManualEdgeSpec {
                        source_name: source_name.to_string(),
                        target_name: name.trim().to_string(),
                        kind: EdgeKind::USAGE,
                        line,
                    });
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_tsx_jsx_targets(Some(child), source_name, source, line, edges);
    }
}

fn unique_node_id_by_name<F>(
    nodes: &HashMap<NodeId, Node>,
    name: &str,
    predicate: F,
) -> Option<NodeId>
where
    F: Fn(NodeKind) -> bool,
{
    let mut matches = nodes
        .values()
        .filter(|node| predicate(node.kind))
        .filter(|node| {
            node.serialized_name == name
                || short_member_name(&node.serialized_name) == name
                || node
                    .qualified_name
                    .as_deref()
                    .map(|qualified_name| qualified_name == name || short_member_name(qualified_name) == name)
                    .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.start_line
            .unwrap_or(u32::MAX)
            .cmp(&right.start_line.unwrap_or(u32::MAX))
            .then_with(|| node_span_width(right).cmp(&node_span_width(left)))
            .then_with(|| left.id.cmp(&right.id))
    });
    matches.first().map(|node| node.id)
}

fn append_manual_type_argument_edges(
    language_name: &str,
    tree: &Tree,
    source: &str,
    unique_nodes: &HashMap<NodeId, Node>,
    file_id: NodeId,
    result_edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
) {
    let specs = match language_name {
        "rust" => collect_rust_generic_type_argument_edges(tree, source),
        "cpp" => collect_cpp_template_type_argument_edges(tree, source),
        _ => Vec::new(),
    };

    for spec in specs {
        let source_id = match spec.kind {
            EdgeKind::CALL => unique_node_id_by_name(unique_nodes, &spec.source_name, |kind| {
                matches!(kind, NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO)
            }),
            EdgeKind::TYPE_ARGUMENT if language_name == "rust" => {
                unique_node_id_by_name(unique_nodes, &spec.source_name, |kind| {
                    matches!(kind, NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO)
                })
            }
            _ => unique_node_id_by_name(unique_nodes, &spec.source_name, is_type_like_kind),
        };
        let Some(source_id) = source_id else {
            continue;
        };
        let target_id = match spec.kind {
            EdgeKind::CALL => unique_node_id_by_name(unique_nodes, &spec.target_name, |kind| {
                matches!(kind, NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO)
            }),
            _ => unique_node_id_by_name(unique_nodes, &spec.target_name, is_type_like_kind),
        };
        let Some(target_id) = target_id else {
            continue;
        };

        let mut edge = Edge {
            id: EdgeId(0),
            source: source_id,
            target: target_id,
            kind: spec.kind,
            file_node_id: Some(file_id),
            line: spec.line,
            ..Default::default()
        };
        if edge.kind == EdgeKind::CALL && !flags.legacy_edge_identity {
            ensure_callsite_identity(&mut edge, None);
        }
        if !edge_keys.insert(edge_dedup_key(&edge, flags)) {
            continue;
        }
        edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
        result_edges.push(edge);
    }
}

fn append_manual_usage_edges(
    is_tsx_file: bool,
    tree: &Tree,
    source: &str,
    unique_nodes: &HashMap<NodeId, Node>,
    file_id: NodeId,
    result_edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
) {
    if !is_tsx_file {
        return;
    }

    for spec in collect_tsx_jsx_usage_edges(tree, source) {
        let Some(source_id) = unique_node_id_by_name(unique_nodes, &spec.source_name, |kind| {
            matches!(kind, NodeKind::FUNCTION | NodeKind::METHOD)
        }) else {
            continue;
        };
        let Some(target_id) = unique_node_id_by_name(unique_nodes, &spec.target_name, |kind| {
            matches!(kind, NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::FIELD)
        }) else {
            continue;
        };

        let mut edge = Edge {
            id: EdgeId(0),
            source: source_id,
            target: target_id,
            kind: EdgeKind::USAGE,
            file_node_id: Some(file_id),
            line: spec.line,
            ..Default::default()
        };
        if !edge_keys.insert(edge_dedup_key(&edge, flags)) {
            continue;
        }
        edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
        result_edges.push(edge);
    }
}

fn infer_access_from_source(
    language_name: &str,
    tree: &Tree,
    source: &str,
    start_line: u32,
    kind: NodeKind,
) -> Option<AccessKind> {
    if !matches!(
        kind,
        NodeKind::METHOD
            | NodeKind::FIELD
            | NodeKind::VARIABLE
            | NodeKind::GLOBAL_VARIABLE
            | NodeKind::CONSTANT
    ) {
        return None;
    }

    if let Some(line_text) = source_line(source, start_line) {
        let access = match language_name {
            "rust" => classify_rust_visibility(line_text),
            _ => classify_keyword_access(line_text),
        };
        if access.is_some() {
            return access;
        }
    }
    if let Some(prev_line) = start_line.checked_sub(1).and_then(|line| source_line(source, line)) {
        let access = match language_name {
            "rust" => classify_rust_visibility(prev_line),
            _ => classify_keyword_access(prev_line),
        };
        if access.is_some() {
            return access;
        }
    }

    match language_name {
        "rust" => Some(AccessKind::Private),
        "java" => Some(AccessKind::Default),
        "typescript" | "javascript" => Some(AccessKind::Public),
        "cpp" | "c" => infer_cpp_access_from_tree(tree, source, start_line).or_else(|| {
            let lines: Vec<&str> = source.lines().collect();
            let mut idx = start_line.saturating_sub(1) as i32;
            let mut remaining = 40;
            while idx >= 0 && remaining > 0 {
                let line = lines[idx as usize].trim().to_ascii_lowercase();
                if line.starts_with("public:") {
                    return Some(AccessKind::Public);
                }
                if line.starts_with("protected:") {
                    return Some(AccessKind::Protected);
                }
                if line.starts_with("private:") {
                    return Some(AccessKind::Private);
                }
                if line.contains("struct ") {
                    return Some(AccessKind::Public);
                }
                if line.contains("class ") {
                    return Some(AccessKind::Private);
                }
                idx -= 1;
                remaining -= 1;
            }
            Some(AccessKind::Private)
        }),
        _ => Some(AccessKind::Public),
    }
}

fn definition_occurrences(
    unique_nodes: &HashMap<NodeId, Node>,
    file_id: NodeId,
) -> Vec<Occurrence> {
    let mut occurrences = Vec::new();
    for node in unique_nodes.values() {
        if let (Some(start_line), Some(start_col), Some(end_line), Some(end_col)) =
            (node.start_line, node.start_col, node.end_line, node.end_col)
        {
            occurrences.push(Occurrence {
                element_id: node.id.0,
                kind: codestory_core::OccurrenceKind::DEFINITION,
                location: SourceLocation {
                    file_node_id: file_id,
                    start_line,
                    start_col,
                    end_line,
                    end_col,
                },
            });
        }
    }

    occurrences
}

fn apply_qualified_names(nodes: Vec<Node>, edges: &[Edge], language_name: &str) -> Vec<Node> {
    let mut parent_map: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut has_parent: HashMap<NodeId, bool> = HashMap::new();

    for edge in edges {
        if edge.kind == EdgeKind::MEMBER {
            parent_map.entry(edge.source).or_default().push(edge.target);
            has_parent.insert(edge.target, true);
        }
    }

    let mut node_map: HashMap<NodeId, Node> = nodes.into_iter().map(|n| (n.id, n)).collect();
    let mut queue: Vec<(NodeId, String)> = Vec::new();

    for id in node_map.keys() {
        if !has_parent.contains_key(id)
            && let Some(node) = node_map.get(id)
        {
            queue.push((*id, node.serialized_name.clone()));
        }
    }

    while let Some((parent_id, parent_qualified_name)) = queue.pop() {
        if let Some(children) = parent_map.get(&parent_id) {
            for child_id in children {
                if let Some(child_node) = node_map.get_mut(child_id) {
                    let delimiter = match language_name {
                        "rust" | "cpp" | "c" => "::",
                        _ => ".",
                    };
                    let new_name = format!(
                        "{}{}{}",
                        parent_qualified_name, delimiter, child_node.serialized_name
                    );
                    child_node.serialized_name = new_name.clone();
                    queue.push((*child_id, new_name));
                }
            }
        }
    }

    node_map.into_values().collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanonicalNodeRole {
    Declaration,
    ImplAnchor,
    Unspecified,
}

fn canonical_role_from_graph_attr(value: &str) -> CanonicalNodeRole {
    match value {
        "declaration" => CanonicalNodeRole::Declaration,
        "impl_anchor" => CanonicalNodeRole::ImplAnchor,
        _ => CanonicalNodeRole::Unspecified,
    }
}

fn canonical_role_priority(role: CanonicalNodeRole) -> u8 {
    match role {
        CanonicalNodeRole::Declaration => 2,
        CanonicalNodeRole::Unspecified => 1,
        CanonicalNodeRole::ImplAnchor => 0,
    }
}

fn is_type_like_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::CLASS
            | NodeKind::STRUCT
            | NodeKind::INTERFACE
            | NodeKind::UNION
            | NodeKind::ENUM
            | NodeKind::TYPEDEF
            | NodeKind::TYPE_PARAMETER
            | NodeKind::BUILTIN_TYPE
            | NodeKind::ANNOTATION
    )
}

fn type_anchor_priority(kind: NodeKind) -> u8 {
    match kind {
        NodeKind::STRUCT => 7,
        NodeKind::ENUM => 6,
        NodeKind::INTERFACE => 5,
        NodeKind::UNION => 4,
        NodeKind::TYPEDEF => 3,
        NodeKind::CLASS => 2,
        NodeKind::TYPE_PARAMETER | NodeKind::ANNOTATION | NodeKind::BUILTIN_TYPE => 1,
        _ => 0,
    }
}

fn node_span_width(node: &Node) -> u32 {
    let start_line = node.start_line.unwrap_or(u32::MAX);
    let end_line = node.end_line.unwrap_or(start_line);
    let start_col = node.start_col.unwrap_or(u32::MAX);
    let end_col = node.end_col.unwrap_or(start_col);
    end_line
        .saturating_sub(start_line)
        .saturating_mul(1_000)
        .saturating_add(end_col.saturating_sub(start_col))
}

fn compare_canonical_node_candidates(
    left: &Node,
    right: &Node,
    canonical_roles: &HashMap<NodeId, CanonicalNodeRole>,
) -> std::cmp::Ordering {
    let left_role = canonical_roles
        .get(&left.id)
        .copied()
        .unwrap_or(CanonicalNodeRole::Unspecified);
    let right_role = canonical_roles
        .get(&right.id)
        .copied()
        .unwrap_or(CanonicalNodeRole::Unspecified);

    canonical_role_priority(left_role)
        .cmp(&canonical_role_priority(right_role))
        .then_with(|| type_anchor_priority(left.kind).cmp(&type_anchor_priority(right.kind)))
        .then_with(|| {
            right
                .start_line
                .unwrap_or(u32::MAX)
                .cmp(&left.start_line.unwrap_or(u32::MAX))
        })
        .then_with(|| {
            right
                .start_col
                .unwrap_or(u32::MAX)
                .cmp(&left.start_col.unwrap_or(u32::MAX))
        })
        .then_with(|| node_span_width(right).cmp(&node_span_width(left)))
        .then_with(|| right.serialized_name.cmp(&left.serialized_name))
}

fn canonicalize_nodes(
    file_name: &str,
    final_nodes: Vec<Node>,
    canonical_roles: &HashMap<NodeId, CanonicalNodeRole>,
) -> (Vec<Node>, HashMap<NodeId, NodeId>) {
    let mut id_remap = HashMap::<NodeId, NodeId>::new();
    let mut grouped_nodes = BTreeMap::<String, Vec<Node>>::new();

    for mut node in final_nodes {
        let qualified_name = node.serialized_name.clone();
        node.qualified_name = Some(qualified_name.clone());

        let canonical_id = if is_type_like_kind(node.kind) {
            format!("{}:{}", file_name, qualified_name)
        } else {
            let start_line = node.start_line.unwrap_or(1);
            format!("{}:{}:{}", file_name, qualified_name, start_line)
        };
        grouped_nodes.entry(canonical_id).or_default().push(node);
    }

    let mut deduped_nodes = Vec::with_capacity(grouped_nodes.len());
    for (canonical_id, nodes) in grouped_nodes {
        let new_id = NodeId(generate_id(&canonical_id));
        for node in &nodes {
            id_remap.insert(node.id, new_id);
        }

        let mut node = nodes
            .into_iter()
            .max_by(|left, right| compare_canonical_node_candidates(left, right, canonical_roles))
            .unwrap_or_default();
        let selected_role = canonical_roles
            .get(&node.id)
            .copied()
            .unwrap_or(CanonicalNodeRole::Unspecified);
        node.id = new_id;
        node.canonical_id = Some(if selected_role == CanonicalNodeRole::ImplAnchor {
            format!("impl_anchor:{canonical_id}")
        } else {
            canonical_id
        });
        deduped_nodes.push(node);
    }

    (deduped_nodes, id_remap)
}

fn remap_file_affinity(nodes: &mut [Node], new_file_id: NodeId) {
    for node in nodes.iter_mut() {
        node.file_node_id = Some(new_file_id);
    }
}

fn remap_edges(
    edges: &mut [Edge],
    new_file_id: NodeId,
    id_remap: &HashMap<NodeId, NodeId>,
    flags: IndexFeatureFlags,
) {
    for edge in edges.iter_mut() {
        if let Some(new_id) = id_remap.get(&edge.source) {
            edge.source = *new_id;
        }
        if let Some(new_id) = id_remap.get(&edge.target) {
            edge.target = *new_id;
        }
        edge.file_node_id = Some(new_file_id);
        if !flags.legacy_edge_identity {
            ensure_callsite_identity(edge, None);
        }
        edge.id = EdgeId(generate_edge_id_for_edge(edge, flags));
    }
}

fn remap_occurrences(occurrences: &mut [Occurrence], id_remap: &HashMap<NodeId, NodeId>) {
    for occ in occurrences.iter_mut() {
        if let Some(new_id) = id_remap.get(&NodeId(occ.element_id)) {
            occ.element_id = new_id.0;
        }
        if let Some(new_file_id) = id_remap.get(&occ.location.file_node_id) {
            occ.location.file_node_id = *new_file_id;
        }
    }
}

fn short_member_name(name: &str) -> &str {
    let colon = name.rfind("::").map(|idx| idx + 2).unwrap_or(0);
    let dot = name.rfind('.').map(|idx| idx + 1).unwrap_or(0);
    let split = colon.max(dot);
    &name[split..]
}

fn rewrite_override_placeholders(file_id: NodeId, nodes: &mut Vec<Node>, edges: &mut [Edge]) {
    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node.clone()))
        .collect::<HashMap<_, _>>();
    let mut synthetic_nodes = Vec::new();
    let mut placeholder_by_source = HashMap::<NodeId, NodeId>::new();

    for edge in edges.iter_mut().filter(|edge| edge.kind == EdgeKind::OVERRIDE) {
        if edge.source != edge.target {
            continue;
        }
        let Some(source_node) = node_by_id.get(&edge.source) else {
            continue;
        };
        let placeholder_id = *placeholder_by_source.entry(edge.source).or_insert_with(|| {
            let method_name = short_member_name(&source_node.serialized_name);
            let canonical_seed = format!(
                "override:{}:{}:{}",
                file_id.0,
                source_node.id.0,
                source_node.start_line.unwrap_or(0)
            );
            let node_id = NodeId(generate_id(&canonical_seed));
            synthetic_nodes.push(Node {
                id: node_id,
                kind: NodeKind::METHOD,
                serialized_name: format!("override::{method_name}"),
                qualified_name: Some(format!("override::{method_name}")),
                canonical_id: Some(canonical_seed),
                file_node_id: Some(file_id),
                start_line: source_node.start_line,
                start_col: source_node.start_col,
                end_line: source_node.end_line,
                end_col: source_node.end_col,
            });
            node_id
        });
        edge.target = placeholder_id;
    }

    if !synthetic_nodes.is_empty() {
        nodes.extend(synthetic_nodes);
    }
}

fn reconcile_tsx_usage_targets(nodes: &[Node], edges: &mut [Edge]) {
    let node_by_id = nodes.iter().map(|node| (node.id, node)).collect::<HashMap<_, _>>();
    let mut best_by_key = HashMap::<(NodeKind, String), NodeId>::new();
    for node in nodes {
        let key = (node.kind, short_member_name(&node.serialized_name).to_string());
        let replace = best_by_key
            .get(&key)
            .and_then(|current_id| node_by_id.get(current_id))
            .map(|current| {
                node.start_line
                    .unwrap_or(u32::MAX)
                    .cmp(&current.start_line.unwrap_or(u32::MAX))
                    .then_with(|| node_span_width(current).cmp(&node_span_width(node)))
                    .is_lt()
            })
            .unwrap_or(true);
        if replace {
            best_by_key.insert(key, node.id);
        }
    }

    for edge in edges.iter_mut().filter(|edge| edge.kind == EdgeKind::USAGE) {
        let Some(target_node) = node_by_id.get(&edge.target).copied() else {
            continue;
        };
        let key = (
            target_node.kind,
            short_member_name(&target_node.serialized_name).to_string(),
        );
        let Some(candidate_id) = best_by_key.get(&key).copied() else {
            continue;
        };
        edge.target = candidate_id;
        if edge.resolved_target.is_some() {
            edge.resolved_target = Some(candidate_id);
        }
    }
}

fn prune_tsx_duplicate_reference_nodes(
    nodes: &mut Vec<Node>,
    edges: &[Edge],
    occurrences: &mut Vec<Occurrence>,
) {
    let referenced_ids = edges
        .iter()
        .flat_map(|edge| {
            [
                Some(edge.source),
                Some(edge.target),
                edge.resolved_source,
                edge.resolved_target,
            ]
        })
        .flatten()
        .collect::<HashSet<_>>();

    let node_by_id = nodes.iter().map(|node| (node.id, node)).collect::<HashMap<_, _>>();
    let mut best_by_key = HashMap::<(NodeKind, String), NodeId>::new();
    for node in nodes.iter() {
        if !matches!(node.kind, NodeKind::FUNCTION | NodeKind::FIELD) {
            continue;
        }
        let key = (node.kind, short_member_name(&node.serialized_name).to_string());
        let should_replace = best_by_key
            .get(&key)
            .and_then(|current_id| node_by_id.get(current_id))
            .map(|current| {
                node.start_line
                    .unwrap_or(u32::MAX)
                    .cmp(&current.start_line.unwrap_or(u32::MAX))
                    .then_with(|| node_span_width(current).cmp(&node_span_width(node)))
                    .is_lt()
            })
            .unwrap_or(true);
        if should_replace {
            best_by_key.insert(key, node.id);
        }
    }

    let removed_ids = nodes
        .iter()
        .filter_map(|node| {
            if !matches!(node.kind, NodeKind::FUNCTION | NodeKind::FIELD) {
                return None;
            }
            let key = (node.kind, short_member_name(&node.serialized_name).to_string());
            let preferred_id = best_by_key.get(&key).copied()?;
            (preferred_id != node.id && !referenced_ids.contains(&node.id)).then_some(node.id)
        })
        .collect::<HashSet<_>>();

    if removed_ids.is_empty() {
        return;
    }

    nodes.retain(|node| !removed_ids.contains(&node.id));
    occurrences.retain(|occurrence| !removed_ids.contains(&NodeId(occurrence.element_id)));
}

fn post_process_index_results(
    result_nodes: Vec<Node>,
    result_edges: &mut Vec<Edge>,
    result_occurrences: &mut Vec<Occurrence>,
    file_name: &str,
    file_id: NodeId,
    language_name: &str,
    canonical_role_by_node_id: &HashMap<NodeId, CanonicalNodeRole>,
    is_tsx_file: bool,
    flags: IndexFeatureFlags,
) -> (Vec<Node>, NodeId, HashMap<NodeId, NodeId>) {
    // Stage 1: qualify names before deduplication so canonical IDs are stable.
    let final_nodes = apply_qualified_names(result_nodes, result_edges, language_name);
    // Stage 2: canonicalize nodes and capture the remap used by later repair stages.
    let (mut final_nodes, id_remap) =
        canonicalize_nodes(file_name, final_nodes, canonical_role_by_node_id);
    let new_file_id = id_remap.get(&file_id).copied().unwrap_or(file_id);

    // Stage 3: remap nodes, edges, and occurrences to the canonical IDs.
    remap_file_affinity(&mut final_nodes, new_file_id);
    remap_edges(result_edges, new_file_id, &id_remap, flags);
    remap_occurrences(result_occurrences, &id_remap);

    // Stage 4: TSX-only reconciliation runs after remap so it targets canonical nodes.
    if is_tsx_file {
        reconcile_tsx_usage_targets(&final_nodes, result_edges);
        prune_tsx_duplicate_reference_nodes(&mut final_nodes, result_edges, result_occurrences);
    }

    // Stage 5: rewrite override placeholders after remap so synthetic nodes are canonical.
    rewrite_override_placeholders(new_file_id, &mut final_nodes, result_edges);
    // Stage 6: attribute calls to enclosing callables after the structural rewrites settle.
    apply_line_range_call_attribution(&final_nodes, result_edges, flags);

    (final_nodes, new_file_id, id_remap)
}

fn remap_pending_node_id(storage: &mut IntermediateStorage, from: NodeId, to: NodeId) {
    for edge in &mut storage.edges {
        if edge.source == from {
            edge.source = to;
        }
        if edge.target == from {
            edge.target = to;
        }
        if edge.resolved_source == Some(from) {
            edge.resolved_source = Some(to);
        }
        if edge.resolved_target == Some(from) {
            edge.resolved_target = Some(to);
        }
    }

    for occurrence in &mut storage.occurrences {
        if occurrence.element_id == from.0 {
            occurrence.element_id = to.0;
        }
    }

    for (node_id, _) in &mut storage.component_access {
        if *node_id == from {
            *node_id = to;
        }
    }

    for state in &mut storage.callable_projection_states {
        if state.node_id == from {
            state.node_id = to;
        }
    }

    for node_id in &mut storage.impl_anchor_node_ids {
        if *node_id == from {
            *node_id = to;
        }
    }
}

fn rust_type_like_kind_values() -> [i32; 6] {
    [
        NodeKind::STRUCT as i32,
        NodeKind::CLASS as i32,
        NodeKind::INTERFACE as i32,
        NodeKind::ENUM as i32,
        NodeKind::UNION as i32,
        NodeKind::TYPEDEF as i32,
    ]
}

fn choose_pending_impl_anchor_target(
    anchor: &Node,
    nodes: &[Node],
    impl_anchor_ids: &HashSet<NodeId>,
) -> Option<NodeId> {
    let mut matches = nodes
        .iter()
        .filter(|candidate| {
            candidate.id != anchor.id
                && is_type_like_kind(candidate.kind)
                && !impl_anchor_ids.contains(&candidate.id)
                && candidate.serialized_name == anchor.serialized_name
        })
        .map(|candidate| candidate.id)
        .collect::<Vec<_>>();
    matches.sort_unstable();
    matches.dedup();
    if matches.len() == 1 {
        Some(matches[0])
    } else {
        None
    }
}

fn choose_existing_impl_anchor_target(
    storage: &Storage,
    anchor: &Node,
) -> Result<Option<NodeId>> {
    let mut query = String::from(
        "SELECT id, qualified_name
         FROM node
         WHERE serialized_name = ?1
           AND (canonical_id IS NULL OR canonical_id NOT LIKE 'impl_anchor:%')
           AND kind IN (",
    );
    let kind_values = rust_type_like_kind_values();
    for (idx, _) in kind_values.iter().enumerate() {
        if idx > 0 {
            query.push_str(", ");
        }
        query.push('?');
        query.push_str(&(idx + 2).to_string());
    }
    query.push(')');

    let mut stmt = storage
        .get_connection()
        .prepare(&query)
        .map_err(|e| anyhow!("Storage query error: {:?}", e))?;
    let mut params = vec![rusqlite::types::Value::from(anchor.serialized_name.clone())];
    params.extend(
        kind_values
            .iter()
            .copied()
            .map(rusqlite::types::Value::from),
    );
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params), |row| {
            Ok((NodeId(row.get::<_, i64>(0)?), row.get::<_, Option<String>>(1)?))
        })
        .map_err(|e| anyhow!("Storage query error: {:?}", e))?;

    let mut matches = Vec::new();
    for row in rows {
        let (node_id, qualified_name) = row.map_err(|e| anyhow!("Storage row error: {:?}", e))?;
        let _ = qualified_name;
        matches.push(node_id);
    }
    matches.sort_unstable();
    matches.dedup();
    Ok(if matches.len() == 1 {
        Some(matches[0])
    } else {
        None
    })
}

fn reconcile_rust_impl_anchors(
    storage: &Storage,
    pending: &mut IntermediateStorage,
) -> Result<()> {
    if pending.impl_anchor_node_ids.is_empty() {
        return Ok(());
    }

    let impl_anchor_ids = pending
        .impl_anchor_node_ids
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let anchor_ids = pending.impl_anchor_node_ids.clone();
    let mut remaps = Vec::<(NodeId, NodeId)>::new();

    for anchor_id in anchor_ids {
        let Some(anchor) = pending.nodes.iter().find(|node| node.id == anchor_id).cloned() else {
            continue;
        };
        if !is_type_like_kind(anchor.kind) {
            continue;
        }

        let target = choose_pending_impl_anchor_target(&anchor, &pending.nodes, &impl_anchor_ids)
            .or_else(|| choose_existing_impl_anchor_target(storage, &anchor).ok().flatten());
        if let Some(target_id) = target {
            remaps.push((anchor.id, target_id));
        }
    }

    if remaps.is_empty() {
        return Ok(());
    }

    for (from, to) in &remaps {
        remap_pending_node_id(pending, *from, *to);
    }

    let removed_ids = remaps.iter().map(|(from, _)| *from).collect::<HashSet<_>>();
    pending.nodes.retain(|node| !removed_ids.contains(&node.id));
    pending.impl_anchor_node_ids.retain(|node_id| !removed_ids.contains(node_id));
    pending.impl_anchor_node_ids.sort_unstable();
    pending.impl_anchor_node_ids.dedup();

    Ok(())
}

/// Index a file and return the results.
pub fn index_file(
    path: &Path,
    source: &str,
    language_config: &LanguageConfig,
    compilation_info: Option<compilation_database::CompilationInfo>,
    symbol_table: Option<Arc<SymbolTable>>,
) -> Result<IndexResult> {
    let flags = index_feature_flags();
    let is_tsx_file = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("tsx"))
        .unwrap_or(false);

    let mut parser = Parser::new();
    parser
        .set_language(&language_config.language)
        .map_err(|e| anyhow!("Language error: {:?}", e))?;
    let compiled_rules = language_config.compiled_rules()?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse source"))?;
    let mut tag_definitions = extract_tag_definitions(compiled_rules, &tree, source)?;

    let mut variables = Variables::new();
    if let Some(info) = &compilation_info {
        // Inject compilation info into graph variables
        for (name, value) in &info.defines {
            let val = value.as_deref().unwrap_or("1");
            let _ = variables.add(name.as_str().into(), val.into());
        }
    }

    let functions = Functions::stdlib();
    let config = ExecutionConfig::new(&functions, &variables);

    let graph = compiled_rules
        .graph_file
        .execute(&tree, source, &config, &NoCancellation)
        .map_err(|e| anyhow!("Graph execution error: {:?}", e))?;

    let mut result_files = Vec::new();
    let mut result_nodes = Vec::new();
    let mut result_edges = Vec::new();
    let mut result_occurrences = Vec::new();

    // 0. Create file node and FileInfo
    let (file_node, file_name, file_id) = file_node_from_source(path, source);
    result_nodes.push(file_node);

    let modification_time = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|systime| {
            systime
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64
        })
        .unwrap_or(0);

    result_files.push(codestory_storage::FileInfo {
        id: file_id.0,
        path: path.to_path_buf(),
        language: language_config.language_name.to_string(),
        modification_time,
        indexed: true,
        complete: !tree.root_node().has_error(),
        line_count: source.lines().count() as u32,
    });

    // 1. First pass: Create nodes and a temporary mapping from GraphNodeId -> OurNodeId
    let mut graph_to_node_id = HashMap::new();
    let mut unique_nodes: HashMap<NodeId, Node> = HashMap::new();
    let mut component_access_by_node_id: HashMap<NodeId, AccessKind> = HashMap::new();
    let mut canonical_role_by_node_id = HashMap::<NodeId, CanonicalNodeRole>::new();

    for node_id in graph.iter_nodes() {
        let node_data = &graph[node_id];

        let mut kind_str = String::new();
        let mut name_str = String::new();
        let mut start_row: Option<u32> = None;
        let mut start_col: Option<u32> = None;
        let mut end_row: Option<u32> = None;
        let mut end_col: Option<u32> = None;
        let mut access_kind: Option<AccessKind> = None;
        let mut canonical_role = CanonicalNodeRole::Unspecified;

        for (attr, val) in node_data.attributes.iter() {
            match attr.as_str() {
                "kind" => kind_str = val.as_str().unwrap_or("UNKNOWN").to_string(),
                "name" => name_str = val.as_str().unwrap_or("").to_string(),
                "start_row" => start_row = val.as_integer().ok(),
                "start_col" => start_col = val.as_integer().ok(),
                "end_row" => end_row = val.as_integer().ok(),
                "end_col" => end_col = val.as_integer().ok(),
                "access" => {
                    if let Ok(value) = val.as_str() {
                        access_kind = access_kind_from_graph_access(value);
                    }
                }
                "canonical_role" => {
                    if let Ok(value) = val.as_str() {
                        canonical_role = canonical_role_from_graph_attr(value);
                    }
                }
                _ => {}
            }
        }

        if language_config.language_name == "rust"
            && kind_str == "MODULE"
            && is_rust_local_symbol_import_path(&name_str)
        {
            name_str = format!("{name_str} (import)");
        }

        if !kind_str.is_empty() && !name_str.is_empty() {
            let mut kind = node_kind_from_graph_kind(kind_str.as_str());
            if language_config.language_name == "python"
                && kind == NodeKind::VARIABLE
                && is_python_constant_name(&name_str)
            {
                kind = NodeKind::CONSTANT;
            }

            let mut start_line = start_row.map(|v| v + 1).unwrap_or(1);
            let mut start_col_1 = start_col.map(|v| v + 1).unwrap_or(1);
            let mut end_line_1 = end_row.map(|v| v + 1).unwrap_or(start_line);
            let mut end_col_1 = end_col.map(|v| v + 1).unwrap_or(start_col_1);
            if let Some(definition) =
                tag_definitions.take(&name_str, start_line, start_col.map(|v| v + 1))
            {
                kind = definition.kind;
                access_kind = definition.access.or(access_kind);
                if definition.canonical_role != CanonicalNodeRole::Unspecified {
                    canonical_role = definition.canonical_role;
                }
                if definition.key.start_line < start_line {
                    start_line = definition.key.start_line;
                    start_col_1 = definition.key.start_col;
                } else if definition.key.start_line == start_line {
                    start_col_1 = start_col_1.min(definition.key.start_col);
                }
                if definition.end_line > end_line_1 {
                    end_line_1 = definition.end_line;
                    end_col_1 = definition.end_col;
                } else if definition.end_line == end_line_1 {
                    end_col_1 = end_col_1.max(definition.end_col);
                }
            }
            let canonical_seed = format!("{}:{}:{}", file_name, name_str, start_line);
            let nid = NodeId(generate_id(&canonical_seed));
            graph_to_node_id.insert(node_id, nid);
            let effective_access = access_kind
                .or_else(|| {
                    infer_access_from_source(
                        language_config.language_name,
                        &tree,
                        source,
                        start_line,
                        kind,
                    )
                });

            unique_nodes.insert(
                nid,
                Node {
                    id: nid,
                    kind,
                    serialized_name: name_str,
                    start_line: Some(start_line),
                    start_col: Some(start_col_1),
                    end_line: Some(end_line_1),
                    end_col: Some(end_col_1),
                    ..Default::default()
                },
            );
            if canonical_role != CanonicalNodeRole::Unspecified {
                canonical_role_by_node_id.insert(nid, canonical_role);
            }
            if let Some(access) = effective_access {
                component_access_by_node_id.insert(nid, access);
            }

            if let Some(st) = &symbol_table {
                st.insert(nid.0, kind);
            }
        }
    }

    for definition in tag_definitions.into_remaining() {
        let canonical_seed = format!(
            "{}:{}:{}",
            file_name, definition.key.name, definition.key.start_line
        );
        let nid = NodeId(generate_id(&canonical_seed));
        unique_nodes.entry(nid).or_insert_with(|| Node {
            id: nid,
            kind: definition.kind,
            serialized_name: definition.key.name.clone(),
            start_line: Some(definition.key.start_line),
            start_col: Some(definition.key.start_col),
            end_line: Some(definition.end_line),
            end_col: Some(definition.end_col),
            ..Default::default()
        });
        if definition.canonical_role != CanonicalNodeRole::Unspecified {
            canonical_role_by_node_id.insert(nid, definition.canonical_role);
        }
        if let Some(access) = definition.access {
            component_access_by_node_id.insert(nid, access);
        }
        if let Some(st) = &symbol_table {
            st.insert(nid.0, definition.kind);
        }
    }

    if !unique_nodes.is_empty() {
        result_nodes.extend(unique_nodes.values().cloned());
    }

    // 2. Second pass: Create edges using tree-sitter-graph output
    let mut edge_keys: HashSet<EdgeDedupKey> = HashSet::new();
    let mut callsite_ordinals: HashMap<(NodeId, Option<u32>), u32> = HashMap::new();

    for source_ref in graph.iter_nodes() {
        let Some(source_id) = graph_to_node_id.get(&source_ref) else {
            continue;
        };
        let graph_node = &graph[source_ref];
        for (sink_ref, edge) in graph_node.iter_edges() {
            let Some(target_id) = graph_to_node_id.get(&sink_ref) else {
                continue;
            };

            let mut kind: Option<EdgeKind> = None;
            let mut line: Option<u32> = None;
            let mut col: Option<u32> = None;
            let mut callsite_identity: Option<String> = None;

            for (attr, val) in edge.attributes.iter() {
                match attr.as_str() {
                    "kind" => {
                        if let Ok(kind_str) = val.as_str() {
                            kind = edge_kind_from_str(kind_str);
                        }
                    }
                    "line" | "start_row" => {
                        if let Ok(row) = val.as_integer() {
                            line = Some(row + 1);
                        }
                    }
                    "col" | "start_col" | "column" => {
                        if let Ok(raw_col) = val.as_integer() {
                            col = Some(raw_col + 1);
                        }
                    }
                    "callsite_identity" | "callsite_id" | "callsite" => {
                        if let Ok(raw) = val.as_str() {
                            let raw = raw.trim();
                            if !raw.is_empty() {
                                callsite_identity = Some(raw.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }

            let Some(kind) = kind else {
                continue;
            };

            let mut edge = Edge {
                id: EdgeId(0),
                source: *source_id,
                target: *target_id,
                kind,
                file_node_id: Some(file_id),
                line,
                callsite_identity,
                ..Default::default()
            };
            if edge.kind == EdgeKind::CALL
                && !flags.legacy_edge_identity
                && edge.callsite_identity.is_none()
            {
                let resolved_col = col.or_else(|| {
                    let key = (edge.target, edge.line);
                    let next = callsite_ordinals.entry(key).or_insert(0);
                    *next = next.saturating_add(1);
                    Some(*next)
                });
                ensure_callsite_identity(&mut edge, resolved_col);
            }
            if !edge_keys.insert(edge_dedup_key(&edge, flags)) {
                continue;
            }

            edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
            result_edges.push(edge);
        }
    }

    append_manual_type_argument_edges(
        language_config.language_name,
        &tree,
        source,
        &unique_nodes,
        file_id,
        &mut result_edges,
        &mut edge_keys,
        flags,
    );
    append_manual_usage_edges(
        is_tsx_file,
        &tree,
        source,
        &unique_nodes,
        file_id,
        &mut result_edges,
        &mut edge_keys,
        flags,
    );

    result_occurrences.extend(definition_occurrences(&unique_nodes, file_id));

    // 3. Resolve qualified names, canonicalize IDs, and remap projections.
    let (final_nodes, _new_file_id, id_remap) = post_process_index_results(
        result_nodes,
        &mut result_edges,
        &mut result_occurrences,
        &file_name,
        file_id,
        language_config.language_name,
        &canonical_role_by_node_id,
        is_tsx_file,
        flags,
    );
    let final_node_ids = final_nodes.iter().map(|node| node.id).collect::<HashSet<_>>();
    let mut remapped_component_access: HashMap<NodeId, AccessKind> = HashMap::new();
    for (original_id, access) in component_access_by_node_id {
        let remapped_id = id_remap.get(&original_id).copied().unwrap_or(original_id);
        if final_node_ids.contains(&remapped_id) {
            remapped_component_access.insert(remapped_id, access);
        }
    }
    let component_access = remapped_component_access.into_iter().collect::<Vec<_>>();
    let mut impl_anchor_node_ids = canonical_role_by_node_id
        .iter()
        .filter_map(|(node_id, role)| {
            (*role == CanonicalNodeRole::ImplAnchor)
                .then(|| id_remap.get(node_id).copied().unwrap_or(*node_id))
        })
        .collect::<Vec<_>>();
    impl_anchor_node_ids.sort_unstable();
    impl_anchor_node_ids.dedup();

    let callable_projection_states =
        build_callable_projection_states(&final_nodes, &result_edges, &result_occurrences);

    if let Some(st) = &symbol_table {
        for node in &final_nodes {
            st.insert(node.id.0, node.kind);
        }
    }

    Ok(IndexResult {
        files: result_files,
        nodes: final_nodes,
        edges: result_edges,
        occurrences: result_occurrences,
        component_access,
        callable_projection_states,
        impl_anchor_node_ids,
    })
}

pub fn get_language_for_ext(ext: &str) -> Option<LanguageConfig> {
    let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    match ext.as_str() {
        // Keep this extension map aligned with the top-level live rule registry.
        "py" | "pyi" => Some(make_language_config(
            tree_sitter_python::LANGUAGE.into(),
            "python",
            PYTHON_GRAPH_QUERY,
            None,
            LanguageRuleset::Python,
        )),
        "java" => Some(make_language_config(
            tree_sitter_java::LANGUAGE.into(),
            "java",
            JAVA_GRAPH_QUERY,
            None,
            LanguageRuleset::Java,
        )),
        "rs" => Some(make_language_config(
            tree_sitter_rust::LANGUAGE.into(),
            "rust",
            RUST_GRAPH_QUERY,
            Some(RUST_TAGS_QUERY),
            LanguageRuleset::Rust,
        )),
        "js" | "jsx" | "mjs" | "cjs" => Some(make_language_config(
            tree_sitter_javascript::LANGUAGE.into(),
            "javascript",
            JAVASCRIPT_GRAPH_QUERY,
            None,
            LanguageRuleset::JavaScript,
        )),
        "ts" | "mts" | "cts" => Some(make_language_config(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "typescript",
            TYPESCRIPT_GRAPH_QUERY,
            Some(TYPESCRIPT_TAGS_QUERY),
            LanguageRuleset::TypeScript,
        )),
        "tsx" => Some(make_language_config(
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            "typescript",
            TSX_GRAPH_QUERY,
            Some(TSX_TAGS_QUERY),
            LanguageRuleset::Tsx,
        )),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Some(make_language_config(
            tree_sitter_cpp::LANGUAGE.into(),
            "cpp",
            CPP_GRAPH_QUERY,
            None,
            LanguageRuleset::Cpp,
        )),
        "c" | "h" => Some(make_language_config(
            tree_sitter_c::LANGUAGE.into(),
            "c",
            C_GRAPH_QUERY,
            None,
            LanguageRuleset::C,
        )),
        _ => None,
    }
}

pub fn generate_id(name: &str) -> i64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in name.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h as i64
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EdgeDedupKey {
    source: NodeId,
    target: NodeId,
    kind: EdgeKind,
    line: Option<u32>,
    callsite_identity: Option<String>,
}

fn canonical_callsite_identity(
    file_node_id: Option<NodeId>,
    line: Option<u32>,
    col: Option<u32>,
    target: NodeId,
) -> Option<String> {
    let file = file_node_id?;
    let line = line.unwrap_or(0);
    let col = col.unwrap_or(0);
    Some(format!("{}:{}:{}:{}", file.0, line, col, target.0))
}

fn ensure_callsite_identity(edge: &mut Edge, col: Option<u32>) {
    if edge.kind != EdgeKind::CALL || edge.callsite_identity.is_some() {
        return;
    }
    edge.callsite_identity =
        canonical_callsite_identity(edge.file_node_id, edge.line, col, edge.target);
}

fn edge_dedup_key(edge: &Edge, flags: IndexFeatureFlags) -> EdgeDedupKey {
    if edge.kind == EdgeKind::CALL && !flags.legacy_edge_identity {
        EdgeDedupKey {
            source: edge.source,
            target: edge.target,
            kind: edge.kind,
            line: edge.line,
            callsite_identity: edge.callsite_identity.clone(),
        }
    } else {
        EdgeDedupKey {
            source: edge.source,
            target: edge.target,
            kind: edge.kind,
            line: None,
            callsite_identity: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FunctionRange {
    id: NodeId,
    start: u32,
    end: u32,
}

fn is_callable_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    )
}

fn apply_line_range_call_attribution(
    nodes: &[Node],
    edges: &mut Vec<Edge>,
    flags: IndexFeatureFlags,
) {
    let mut functions_by_file: HashMap<NodeId, Vec<FunctionRange>> = HashMap::new();
    let callable_ids: HashSet<NodeId> = nodes
        .iter()
        .filter(|node| is_callable_kind(node.kind))
        .map(|node| node.id)
        .collect();

    for node in nodes {
        if !is_callable_kind(node.kind) {
            continue;
        }
        let (Some(file_id), Some(start), Some(end)) =
            (node.file_node_id, node.start_line, node.end_line)
        else {
            continue;
        };
        if start > end {
            continue;
        }
        functions_by_file
            .entry(file_id)
            .or_default()
            .push(FunctionRange {
                id: node.id,
                start,
                end,
            });
    }

    for ranges in functions_by_file.values_mut() {
        ranges.sort_by_key(|range| (range.end - range.start, range.start));
    }

    let mut dedup: HashSet<EdgeDedupKey> = HashSet::new();
    let mut updated_edges = Vec::with_capacity(edges.len());

    for edge in edges.iter_mut() {
        if edge.kind == EdgeKind::CALL {
            let mut attributed = callable_ids.contains(&edge.source);
            if let (Some(file_id), Some(line)) = (edge.file_node_id, edge.line)
                && let Some(ranges) = functions_by_file.get(&file_id)
                && let Some(best) = ranges
                    .iter()
                    .filter(|range| line >= range.start && line <= range.end)
                    .min_by_key(|range| (range.end - range.start, range.start))
            {
                edge.source = best.id;
                attributed = true;
            }
            if !attributed || edge.source == edge.target {
                continue;
            }
            if !flags.legacy_edge_identity {
                ensure_callsite_identity(edge, None);
            }
        }

        edge.id = EdgeId(generate_edge_id_for_edge(edge, flags));

        if dedup.insert(edge_dedup_key(edge, flags)) {
            updated_edges.push(edge.clone());
        }
    }

    *edges = updated_edges;
}

fn build_callable_projection_states(
    nodes: &[Node],
    edges: &[Edge],
    occurrences: &[Occurrence],
) -> Vec<CallableProjectionState> {
    let mut edges_by_source: HashMap<NodeId, Vec<&Edge>> = HashMap::new();
    for edge in edges {
        edges_by_source.entry(edge.source).or_default().push(edge);
    }

    let mut occurrences_by_file: HashMap<NodeId, Vec<&Occurrence>> = HashMap::new();
    for occurrence in occurrences {
        occurrences_by_file
            .entry(occurrence.location.file_node_id)
            .or_default()
            .push(occurrence);
    }

    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    let mut states = Vec::new();
    for node in nodes {
        if !matches!(
            node.kind,
            NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
        ) {
            continue;
        }
        let (Some(file_id), Some(start_line), Some(start_col), Some(end_line)) = (
            node.file_node_id,
            node.start_line,
            node.start_col,
            node.end_line,
        ) else {
            continue;
        };
        let symbol_key = format!(
            "{}:{}",
            node.kind as i32,
            node.qualified_name
                .as_deref()
                .unwrap_or(node.serialized_name.as_str())
        );
        let signature_hash = hash_parts([
            symbol_key.as_str(),
            &start_line.to_string(),
            &start_col.to_string(),
        ]);

        let mut body_parts = Vec::new();
        if let Some(source_edges) = edges_by_source.get(&node.id) {
            let mut edge_parts = source_edges
                .iter()
                .filter(|edge| {
                    !matches!(
                        edge.kind,
                        EdgeKind::MEMBER
                            | EdgeKind::INHERITANCE
                            | EdgeKind::IMPORT
                            | EdgeKind::OVERRIDE
                    )
                })
                .map(|edge| {
                    format!(
                        "{}:{}:{}:{}",
                        edge.kind as i32,
                        edge.target.0,
                        edge.line.unwrap_or(0),
                        edge.callsite_identity.as_deref().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>();
            edge_parts.sort();
            body_parts.extend(edge_parts);
        }

        if let Some(file_occurrences) = occurrences_by_file.get(&file_id) {
            let mut occurrence_parts = file_occurrences
                .iter()
                .filter(|occurrence| {
                    occurrence.location.start_line >= start_line
                        && occurrence.location.end_line <= end_line
                        && occurrence.element_id != node.id.0
                })
                .map(|occurrence| {
                    format!(
                        "{}:{}:{}:{}:{}:{}",
                        occurrence.element_id,
                        occurrence.kind as i32,
                        occurrence.location.start_line,
                        occurrence.location.start_col,
                        occurrence.location.end_line,
                        occurrence.location.end_col
                    )
                })
                .collect::<Vec<_>>();
            occurrence_parts.sort();
            body_parts.extend(occurrence_parts);
        }

        states.push(CallableProjectionState {
            file_id: file_id.0,
            symbol_key,
            node_id: node.id,
            signature_hash,
            body_hash: hash_parts(body_parts.iter().map(String::as_str)),
            start_line,
            end_line,
        });
    }

    if let Some(file_node) = nodes.iter().find(|node| node.kind == NodeKind::FILE) {
        states.push(CallableProjectionState {
            file_id: file_node.id.0,
            symbol_key: FILE_STRUCTURAL_SYMBOL_KEY.to_string(),
            node_id: file_node.id,
            signature_hash: hash_parts([FILE_STRUCTURAL_SYMBOL_KEY]),
            body_hash: structural_projection_hash(file_node.id, nodes, edges, &node_by_id),
            start_line: 1,
            end_line: file_node.end_line.unwrap_or(1),
        });
    }

    states.sort_by(|lhs, rhs| lhs.symbol_key.cmp(&rhs.symbol_key));
    states
}

fn structural_projection_hash(
    file_id: NodeId,
    nodes: &[Node],
    edges: &[Edge],
    node_by_id: &HashMap<NodeId, &Node>,
) -> i64 {
    let mut parts = Vec::new();

    for node in nodes {
        if node.id == file_id {
            continue;
        }
        if is_callable_kind(node.kind) {
            parts.push(format!(
                "callable:{}:{}",
                node.kind as i32,
                node.qualified_name
                    .as_deref()
                    .unwrap_or(node.serialized_name.as_str())
            ));
            continue;
        }
        parts.push(format!(
            "node:{}:{}:{}",
            node.kind as i32,
            node.qualified_name
                .as_deref()
                .unwrap_or(node.serialized_name.as_str()),
            node.start_line.unwrap_or(0)
        ));
    }

    for edge in edges {
        if matches!(edge.kind, EdgeKind::CALL | EdgeKind::USAGE) {
            continue;
        }
        let source_name = node_by_id
            .get(&edge.source)
            .map(|node| {
                node.qualified_name
                    .as_deref()
                    .unwrap_or(node.serialized_name.as_str())
            })
            .unwrap_or_default();
        let target_name = node_by_id
            .get(&edge.target)
            .map(|node| {
                node.qualified_name
                    .as_deref()
                    .unwrap_or(node.serialized_name.as_str())
            })
            .unwrap_or_default();
        parts.push(format!(
            "edge:{}:{}:{}",
            edge.kind as i32, source_name, target_name
        ));
    }

    parts.sort();
    hash_parts(parts.iter().map(String::as_str))
}

fn classify_projection_update(
    existing: &[CallableProjectionState],
    current: &[CallableProjectionState],
) -> ProjectionUpdateMode {
    if existing.is_empty() {
        return ProjectionUpdateMode::InsertFresh;
    }
    if current.is_empty() {
        return ProjectionUpdateMode::FullReplace;
    }

    let existing_by_key = existing
        .iter()
        .map(|state| (state.symbol_key.as_str(), state))
        .collect::<HashMap<_, _>>();
    let current_by_key = current
        .iter()
        .map(|state| (state.symbol_key.as_str(), state))
        .collect::<HashMap<_, _>>();

    if existing_by_key.len() != current_by_key.len() {
        return ProjectionUpdateMode::FullReplace;
    }
    if existing_by_key
        .keys()
        .any(|symbol_key| !current_by_key.contains_key(symbol_key))
    {
        return ProjectionUpdateMode::FullReplace;
    }

    let mut changed_callers = Vec::new();
    for current_state in current {
        let Some(existing_state) = existing_by_key.get(current_state.symbol_key.as_str()) else {
            return ProjectionUpdateMode::FullReplace;
        };
        if current_state.symbol_key == FILE_STRUCTURAL_SYMBOL_KEY {
            if current_state.body_hash != existing_state.body_hash {
                return ProjectionUpdateMode::FullReplace;
            }
            continue;
        }
        if current_state.signature_hash != existing_state.signature_hash {
            return ProjectionUpdateMode::FullReplace;
        }
        if current_state.body_hash != existing_state.body_hash {
            changed_callers.push(current_state.node_id);
        }
    }

    if changed_callers.is_empty() {
        ProjectionUpdateMode::NoChanges
    } else {
        ProjectionUpdateMode::Delta { changed_callers }
    }
}

fn hash_parts<'a>(parts: impl IntoIterator<Item = &'a str>) -> i64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for part in parts {
        for b in part.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    h as i64
}

fn edge_kind_from_str(kind: &str) -> Option<EdgeKind> {
    match kind {
        "MEMBER" => Some(EdgeKind::MEMBER),
        "TYPE_USAGE" => Some(EdgeKind::TYPE_USAGE),
        "USAGE" => Some(EdgeKind::USAGE),
        "CALL" => Some(EdgeKind::CALL),
        "INHERITANCE" => Some(EdgeKind::INHERITANCE),
        "OVERRIDE" => Some(EdgeKind::OVERRIDE),
        "TYPE_ARGUMENT" => Some(EdgeKind::TYPE_ARGUMENT),
        "TEMPLATE_SPECIALIZATION" => Some(EdgeKind::TEMPLATE_SPECIALIZATION),
        "INCLUDE" => Some(EdgeKind::INCLUDE),
        "IMPORT" => Some(EdgeKind::IMPORT),
        "MACRO_USAGE" => Some(EdgeKind::MACRO_USAGE),
        "ANNOTATION_USAGE" => Some(EdgeKind::ANNOTATION_USAGE),
        "UNKNOWN" => Some(EdgeKind::UNKNOWN),
        _ => None,
    }
}

fn generate_edge_id(source: i64, target: i64, kind: codestory_core::EdgeKind) -> i64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut update = |val: i64| {
        for b in val.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    update(source);
    update(target);
    update(kind as i64);
    h as i64
}

fn generate_edge_id_with_identity(
    source: i64,
    target: i64,
    kind: codestory_core::EdgeKind,
    identity: &str,
) -> i64 {
    let mut h = generate_edge_id(source, target, kind) as u64;
    for b in identity.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h as i64
}

fn generate_edge_id_for_edge(edge: &Edge, flags: IndexFeatureFlags) -> i64 {
    if edge.kind == EdgeKind::CALL
        && !flags.legacy_edge_identity
        && let Some(callsite_identity) = edge.callsite_identity.as_deref()
    {
        return generate_edge_id_with_identity(
            edge.source.0,
            edge.target.0,
            edge.kind,
            callsite_identity,
        );
    }
    generate_edge_id(edge.source.0, edge.target.0, edge.kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[derive(Debug)]
    struct RawGraphContract {
        nodes: HashSet<(String, String)>,
        edges: HashSet<(String, String, String)>,
    }

    fn execute_raw_graph_contract(
        path: &Path,
        source: &str,
        language_config: &LanguageConfig,
    ) -> Result<RawGraphContract> {
        let mut parser = Parser::new();
        parser
            .set_language(&language_config.language)
            .map_err(|e| anyhow!("parser language error: {e}"))?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| anyhow!("parser did not produce a tree"))?;
        let variables = Variables::new();
        let functions = Functions::stdlib();
        let config = ExecutionConfig::new(&functions, &variables);
        let graph = language_config
            .compiled_rules()?
            .graph_file
            .execute(&tree, source, &config, &NoCancellation)
            .map_err(|e| anyhow!("Graph execution error: {:?}", e))?;

        let mut node_names = HashMap::new();
        let mut nodes = HashSet::new();
        for node_id in graph.iter_nodes() {
            let node_data = &graph[node_id];
            let mut kind = None;
            let mut name = None;
            for (attr, val) in node_data.attributes.iter() {
                match attr.as_str() {
                    "kind" => kind = val.as_str().ok().map(str::to_string),
                    "name" => name = val.as_str().ok().map(str::to_string),
                    _ => {}
                }
            }
            let (Some(kind), Some(name)) = (kind, name) else {
                continue;
            };
            node_names.insert(node_id, name.clone());
            nodes.insert((kind, name));
        }

        let mut edges = HashSet::new();
        for source_ref in graph.iter_nodes() {
            let Some(source_name) = node_names.get(&source_ref).cloned() else {
                continue;
            };
            let graph_node = &graph[source_ref];
            for (target_ref, edge) in graph_node.iter_edges() {
                let Some(target_name) = node_names.get(&target_ref).cloned() else {
                    continue;
                };
                let mut kind = None;
                for (attr, val) in edge.attributes.iter() {
                    if attr.as_str() == "kind" {
                        kind = val.as_str().ok().map(str::to_string);
                    }
                }
                let Some(kind) = kind else {
                    continue;
                };
                edges.insert((source_name.clone(), target_name, kind));
            }
        }

        let _ = path;
        Ok(RawGraphContract { nodes, edges })
    }

    fn parser_node_kinds(language: Language) -> HashSet<String> {
        (0..language.node_kind_count())
            .filter_map(|id| language.node_kind_for_id(id as u16))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn test_index_python_semantics() -> Result<()> {
        let _ = tracing_subscriber::fmt::try_init();

        let python_code = r#"
class Parent:
    pass

class MyClass(Parent):
    def my_method(self):
        pass
"#;
        let language_config = get_language_for_ext("py").unwrap();

        let result = index_file(
            Path::new("test.py"),
            python_code,
            &language_config,
            None,
            None,
        )?;

        assert!(
            result.edges.iter().any(|e| e.kind == EdgeKind::MEMBER),
            "MEMBER edge not found"
        );
        assert!(
            result.edges.iter().any(|e| e.kind == EdgeKind::INHERITANCE),
            "INHERITANCE edge not found"
        );
        assert!(!result.occurrences.is_empty(), "No occurrences found");

        Ok(())
    }

    #[test]
    fn test_index_java_semantics() -> Result<()> {
        let java_code = r#"
class Parent {}
class MyClass extends Parent {
    void myMethod() {}
}
"#;
        let language_config = get_language_for_ext("java").unwrap();

        let result = index_file(
            Path::new("Test.java"),
            java_code,
            &language_config,
            None,
            None,
        )?;

        assert!(
            result.edges.iter().any(|e| e.kind == EdgeKind::MEMBER),
            "MEMBER edge not found"
        );
        assert!(
            result.edges.iter().any(|e| e.kind == EdgeKind::INHERITANCE),
            "INHERITANCE edge not found"
        );
        Ok(())
    }

    #[test]
    fn test_index_rust_semantics() -> Result<()> {
        let rust_code = r#"
struct MyStruct { field: i32 }
impl MyStruct {
    fn my_fn(&self) {}
}
"#;
        let language_config = get_language_for_ext("rs").unwrap();

        let result = index_file(
            Path::new("main.rs"),
            rust_code,
            &language_config,
            None,
            None,
        )?;

        assert!(
            result.edges.iter().any(|e| e.kind == EdgeKind::MEMBER),
            "MEMBER edge not found"
        );
        Ok(())
    }

    #[test]
    fn test_rust_type_anchor_prefers_declaration_over_impl_anchor() -> Result<()> {
        let rust_code = r#"
pub struct AppController;

impl Default for AppController {
    fn default() -> Self {
        Self
    }
}

impl AppController {
    fn open_project(&self) {}
}
"#;
        let language_config = get_language_for_ext("rs").unwrap();

        let result = index_file(
            Path::new("main.rs"),
            rust_code,
            &language_config,
            None,
            None,
        )?;

        let matching = result
            .nodes
            .iter()
            .filter(|node| node.serialized_name == "AppController")
            .collect::<Vec<_>>();
        assert_eq!(
            matching.len(),
            1,
            "expected one canonical AppController node"
        );

        let type_node = matching[0];
        assert_eq!(type_node.kind, NodeKind::STRUCT);
        assert_eq!(type_node.start_line, Some(2));

        let open_project = result
            .nodes
            .iter()
            .find(|node| node.serialized_name.ends_with("open_project"))
            .expect("open_project method");
        assert!(result.edges.iter().any(|edge| {
            edge.kind == EdgeKind::MEMBER
                && edge.source == type_node.id
                && edge.target == open_project.id
        }));

        Ok(())
    }

    #[test]
    fn test_index_cpp_semantics() -> Result<()> {
        let cpp_code = r#"
class MyClass {
    void myMethod() {}
};
"#;
        let language_config = get_language_for_ext("cpp").unwrap();

        let result = index_file(
            Path::new("test.cpp"),
            cpp_code,
            &language_config,
            None,
            None,
        )?;

        assert!(
            result.edges.iter().any(|e| e.kind == EdgeKind::MEMBER),
            "MEMBER edge not found"
        );
        Ok(())
    }

    #[test]
    fn test_index_typescript_semantics() -> Result<()> {
        let ts_code = r#"
class MyClass {
    myMethod() {}
}
function globalFunc() {}
"#;
        let language_config = get_language_for_ext("ts").unwrap();

        let result = index_file(
            Path::new("test.ts"),
            ts_code,
            &language_config,
            None,
            None,
        )?;

        // Find MyClass
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name == "MyClass" && n.kind == NodeKind::CLASS)
        );
        // Find globalFunc
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name == "globalFunc" && n.kind == NodeKind::FUNCTION)
        );

        // Assert Edge Creation (MEMBER)
        // Note: The original query for TS likely failed to match class name which is type_identifier
        assert!(
            result.edges.iter().any(|e| e.kind == EdgeKind::MEMBER),
            "MEMBER edge not found in TypeScript index result"
        );

        Ok(())
    }

    #[test]
    fn test_header_language_defaults_to_c_and_can_upgrade_to_cpp_from_compile_info() {
        let default_config = get_language_for_ext("h").expect("header extension should resolve");
        assert_eq!(default_config.language_name, "c");

        let cpp_info = compilation_database::CompilationInfo {
            standard: Some(compilation_database::CxxStandard::Cxx20),
            ..Default::default()
        };
        let config = get_language_config_for_path(Path::new("widget.h"), Some(&cpp_info))
            .expect("path-based header config should resolve");
        assert_eq!(config.language_name, "cpp");
    }

    #[test]
    fn test_file_completeness_tracks_parse_errors() -> Result<()> {
        let language_config = get_language_for_ext("rs").unwrap();
        let result = index_file(
            Path::new("broken.rs"),
            "fn broken( {",
            &language_config,
            None,
            None,
        )?;

        assert_eq!(result.files.len(), 1);
        assert!(!result.files[0].complete, "malformed Rust source should be incomplete");
        Ok(())
    }

    #[test]
    fn test_incremental_indexing() -> Result<()> {
        use codestory_project::RefreshInfo;
        use codestory_storage::Storage;
        use std::fs;
        use std::time::{Duration, Instant};
        use tempfile::tempdir;

        let dir = tempdir()?;
        let f1 = dir.path().join("main.rs");
        fs::write(
            &f1,
            r#"
            struct Foo { x: i32 }
            fn bar() {}
        "#,
        )?;

        let mut storage = Storage::new_in_memory().unwrap();
        let bus = EventBus::new();
        let rx = bus.receiver();
        let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());

        // Create RefreshInfo manually
        let refresh_info = RefreshInfo {
            files_to_index: vec![f1.clone()],
            files_to_remove: vec![],
        };

        indexer.run_incremental(&mut storage, &refresh_info, &bus, None)?;

        // Check verification
        let nodes = storage.get_nodes().unwrap();
        assert!(
            nodes
                .iter()
                .any(|n| n.serialized_name == "Foo" && n.kind == NodeKind::STRUCT)
        );
        assert!(
            nodes
                .iter()
                .any(|n| n.serialized_name == "bar" && n.kind == NodeKind::FUNCTION)
        );

        // Check progress events with a short timeout to avoid race with async fan-out thread.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_started = false;
        let mut saw_complete = false;
        while Instant::now() < deadline && (!saw_started || !saw_complete) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::IndexingStarted { .. }) => saw_started = true,
                Ok(Event::IndexingComplete { .. }) => saw_complete = true,
                Ok(_) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(saw_started, "expected IndexingStarted event");
        assert!(saw_complete, "expected IndexingComplete event");

        Ok(())
    }

    #[test]
    fn test_incremental_indexing_batch_flush() -> Result<()> {
        use codestory_project::RefreshInfo;
        use codestory_storage::Storage;
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir()?;
        let mut files = Vec::new();
        for index in 0..12 {
            let path = dir.path().join(format!("module_{index}.rs"));
            fs::write(&path, format!("struct File_{index} {{}}\n"))?;
            files.push(path);
        }

        let mut storage = Storage::new_in_memory().unwrap();
        let bus = EventBus::new();
        let indexer = WorkspaceIndexer::new(dir.path().to_path_buf()).with_batch_config(
            IncrementalIndexingConfig {
                file_batch_size: 3,
                node_batch_size: 4,
                edge_batch_size: 4,
                occurrence_batch_size: 8,
                error_batch_size: 128,
            },
        );

        let refresh_info = RefreshInfo {
            files_to_index: files,
            files_to_remove: vec![],
        };

        indexer.run_incremental(&mut storage, &refresh_info, &bus, None)?;

        // Each file should contribute at least one file node and one symbol node.
        let nodes = storage.get_nodes()?;
        assert!(nodes.len() >= 24);

        Ok(())
    }

    #[test]
    fn test_run_incremental_helper_calls_are_indexed() -> Result<()> {
        use codestory_project::RefreshInfo;
        use codestory_storage::Storage;
        use std::collections::HashSet;
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir()?;
        let f1 = dir.path().join("indexer.rs");
        fs::write(
            &f1,
            r#"
            struct WorkspaceIndexer;
            impl WorkspaceIndexer {
                fn run_incremental(&self) {
                    Self::seed_symbol_table();
                    Self::flush_projection_batch();
                    Self::flush_errors();
                }
                fn seed_symbol_table() {}
                fn flush_projection_batch() {}
                fn flush_errors() {}
            }
        "#,
        )?;

        let mut storage = Storage::new_in_memory().unwrap();
        let bus = EventBus::new();
        let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
        let refresh_info = RefreshInfo {
            files_to_index: vec![f1.clone()],
            files_to_remove: vec![],
        };

        indexer.run_incremental(&mut storage, &refresh_info, &bus, None)?;

        let run_node_ids: HashSet<_> = storage
            .get_nodes()?
            .into_iter()
            .filter(|node| node.serialized_name.ends_with("run_incremental"))
            .map(|node| node.id)
            .collect();
        assert!(!run_node_ids.is_empty(), "run_incremental node not found");

        let edges = storage.get_edges()?;
        let mut callees = HashSet::new();
        for edge in edges {
            if edge.kind != EdgeKind::CALL || !run_node_ids.contains(&edge.source) {
                continue;
            }
            if let Some(callsite_identity) = edge.callsite_identity.as_ref() {
                if !callsite_identity.is_empty() {
                    callees.insert(callsite_identity.clone());
                }
            }
            if let Some(target) = storage.get_node(edge.target)? {
                callees.insert(target.serialized_name);
            }
        }

        assert!(
            callees
                .iter()
                .any(|name| name.contains("seed_symbol_table")),
            "missing seed_symbol_table call edge; found: {:?}",
            callees
        );
        assert!(
            callees
                .iter()
                .any(|name| name.contains("flush_projection_batch")),
            "missing flush_projection_batch call edge; found: {:?}",
            callees
        );
        assert!(
            callees.iter().any(|name| name.contains("flush_errors")),
            "missing flush_errors call edge; found: {:?}",
            callees
        );

        Ok(())
    }

    #[test]
    fn test_index_cpp_advanced() -> Result<()> {
        let code = r#"
class Base {};
class Derived : public Base {
    int x;
    void foo() {}
};
"#;
        let language_config = get_language_for_ext("cpp").unwrap();
        let result = index_file(
            Path::new("test.cpp"),
            code,
            &language_config,
            None,
            None,
        )?;

        // Verify Membership
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name == "Base" && n.kind == NodeKind::CLASS)
        );
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name == "Derived" && n.kind == NodeKind::CLASS)
        );
        // Verify Membership
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::MEMBER));
        // Verify Inheritance (TODO: Fix structural matching for inheritance in single-pass TS queries)
        // assert!(result.edges.iter().any(|e| e.kind == EdgeKind::INHERITANCE));
        Ok(())
    }

    #[test]
    fn test_index_python_advanced() -> Result<()> {
        let code = r#"
from os import path
@decorator
class MyClass:
    x = 1
"#;
        let language_config = get_language_for_ext("py").unwrap();
        let result = index_file(
            Path::new("test.py"),
            code,
            &language_config,
            None,
            None,
        )?;

        // Verify Assignment Node
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name == "x" && n.kind == NodeKind::VARIABLE)
        );
        // Verify IMPORT for import statement
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::IMPORT));
        // Verify USAGE for decorator
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::USAGE));
        Ok(())
    }

    #[test]
    fn test_index_rust_advanced() -> Result<()> {
        let code = r#"
trait MyTrait {}
struct MyStruct;
impl MyTrait for MyStruct {}
fn main() {
    println!("Hello");
}
"#;
        let language_config = get_language_for_ext("rs").unwrap();
        let result = index_file(
            Path::new("main.rs"),
            code,
            &language_config,
            None,
            None,
        )?;

        // Verify Trait Node
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name == "MyTrait" && n.kind == NodeKind::INTERFACE)
        );
        // Verify Impl Inheritance
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::INHERITANCE));
        // Verify Macro usage
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::USAGE));
        Ok(())
    }

    #[test]
    fn test_index_rust_trait_impl_for_generic_type() -> Result<()> {
        let code = r#"
trait Listener {
    fn on_event(&mut self);
}

struct Wrapper<T> {
    inner: T,
}

impl<T> Listener for Wrapper<T> {
    fn on_event(&mut self) {}
}
"#;
        let language_config = get_language_for_ext("rs").unwrap();
        let result = index_file(
            Path::new("main.rs"),
            code,
            &language_config,
            None,
            None,
        )?;

        let listener = result
            .nodes
            .iter()
            .find(|n| n.serialized_name == "Listener" && n.kind == NodeKind::INTERFACE)
            .expect("Listener interface not found");
        let wrapper = result
            .nodes
            .iter()
            .find(|n| n.serialized_name == "Wrapper" && n.kind == NodeKind::STRUCT)
            .unwrap_or_else(|| {
                panic!(
                    "Wrapper type not found; nodes={:?}",
                    result
                        .nodes
                        .iter()
                        .map(|n| (&n.serialized_name, &n.kind))
                        .collect::<Vec<_>>()
                )
            });

        assert!(
            result.edges.iter().any(|e| e.kind == EdgeKind::INHERITANCE
                && e.source == wrapper.id
                && e.target == listener.id),
            "INHERITANCE edge from Wrapper to Listener not found"
        );

        Ok(())
    }

    #[test]
    fn test_index_rust_local_binding_and_closure_assignment_distinguish_variable_and_function()
    -> Result<()> {
        let code = r#"
fn sample(value: i32) -> i32 {
    let local = value + 1;
    let helper = |input: i32| input + local;
    helper(value)
}
"#;
        let language_config = get_language_for_ext("rs").unwrap();
        let result = index_file(
            Path::new("main.rs"),
            code,
            &language_config,
            None,
            None,
        )?;

        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name == "local" && n.kind == NodeKind::VARIABLE),
            "plain let binding should be indexed as VARIABLE"
        );
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name == "helper" && n.kind == NodeKind::FUNCTION),
            "closure-backed let binding should be indexed as FUNCTION"
        );

        Ok(())
    }

    #[test]
    fn test_call_edges_from_graph() -> Result<()> {
        let java_code = r#"
class Test {
    void caller() {
        callee();
    }
    void callee() {}
}
"#;
        let language_config = get_language_for_ext("java").unwrap();
        let result = index_file(
            Path::new("Test.java"),
            java_code,
            &language_config,
            None,
            None,
        )?;

        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name.ends_with(".caller") && n.kind == NodeKind::METHOD),
            "Caller node not found"
        );
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name.ends_with(".callee") && n.kind == NodeKind::METHOD),
            "Callee node not found"
        );
        assert!(
            result.edges.iter().any(|e| e.kind == EdgeKind::CALL),
            "CALL edge not found"
        );

        Ok(())
    }

    #[test]
    fn test_call_attribution_line_range() -> Result<()> {
        let java_code = r#"
class Test {
    void first() {}
    void second() {
        first();
    }
}
"#;
        let language_config = get_language_for_ext("java").unwrap();
        let result = index_file(
            Path::new("Test.java"),
            java_code,
            &language_config,
            None,
            None,
        )?;

        let caller = result
            .nodes
            .iter()
            .find(|n| n.serialized_name.ends_with(".second"))
            .expect("second() node not found");

        let call_edge = result
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::CALL)
            .expect("CALL edge not found");

        assert_eq!(call_edge.source, caller.id);
        Ok(())
    }

    #[test]
    fn test_call_edges_same_line_preserve_distinct_callsites() {
        use std::collections::{HashMap, HashSet};

        let flags = IndexFeatureFlags {
            legacy_edge_identity: false,
        };
        let file_id = NodeId(1);
        let mut edges = vec![
            Edge {
                id: EdgeId(0),
                source: NodeId(10),
                target: NodeId(20),
                kind: EdgeKind::CALL,
                file_node_id: Some(file_id),
                line: Some(42),
                ..Default::default()
            },
            Edge {
                id: EdgeId(0),
                source: NodeId(10),
                target: NodeId(20),
                kind: EdgeKind::CALL,
                file_node_id: Some(file_id),
                line: Some(42),
                ..Default::default()
            },
        ];

        let mut callsite_ordinals: HashMap<(NodeId, Option<u32>), u32> = HashMap::new();
        for edge in &mut edges {
            let key = (edge.target, edge.line);
            let next = callsite_ordinals.entry(key).or_insert(0);
            *next = next.saturating_add(1);
            ensure_callsite_identity(edge, Some(*next));
            edge.id = EdgeId(generate_edge_id_for_edge(edge, flags));
        }

        let mut dedup = HashSet::new();
        let deduped = edges
            .into_iter()
            .filter(|edge| dedup.insert(edge_dedup_key(edge, flags)))
            .collect::<Vec<_>>();

        assert_eq!(deduped.len(), 2, "expected one edge per callsite");
        let identities = deduped
            .iter()
            .map(|edge| edge.callsite_identity.clone().unwrap_or_default())
            .collect::<HashSet<_>>();
        assert_eq!(
            identities.len(),
            2,
            "callsites should have unique identities"
        );
        let edge_ids = deduped.iter().map(|edge| edge.id).collect::<HashSet<_>>();
        assert_eq!(edge_ids.len(), 2, "callsites should have unique edge ids");
    }

    #[test]
    fn test_legacy_edge_identity_dedup_ignores_callsite_identity() {
        let edge_a = Edge {
            id: EdgeId(1),
            source: NodeId(10),
            target: NodeId(20),
            kind: EdgeKind::CALL,
            line: Some(42),
            callsite_identity: Some("10:42:1:20".to_string()),
            ..Default::default()
        };
        let edge_b = Edge {
            id: EdgeId(2),
            source: NodeId(10),
            target: NodeId(20),
            kind: EdgeKind::CALL,
            line: Some(42),
            callsite_identity: Some("10:42:2:20".to_string()),
            ..Default::default()
        };

        let modern_flags = IndexFeatureFlags {
            legacy_edge_identity: false,
        };
        let legacy_flags = IndexFeatureFlags {
            legacy_edge_identity: true,
        };
        assert_ne!(
            edge_dedup_key(&edge_a, modern_flags),
            edge_dedup_key(&edge_b, modern_flags),
            "modern identity should differentiate callsites"
        );
        assert_eq!(
            edge_dedup_key(&edge_a, legacy_flags),
            edge_dedup_key(&edge_b, legacy_flags),
            "legacy identity should collapse callsites"
        );
    }

    #[test]
    fn test_run_incremental_emits_compile_db_warning_on_load_failure() -> Result<()> {
        use codestory_project::RefreshInfo;
        use codestory_storage::Storage;
        use std::fs;
        use std::time::Duration;
        use tempfile::tempdir;

        let dir = tempdir()?;
        fs::write(
            dir.path().join("compile_commands.json"),
            "{ this is not valid json ",
        )?;
        let file = dir.path().join("main.rs");
        fs::write(&file, "fn main() {}")?;

        let mut storage = Storage::new_in_memory().unwrap();
        let bus = EventBus::new();
        let rx = bus.receiver();
        let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
        let refresh_info = RefreshInfo {
            files_to_index: vec![file],
            files_to_remove: vec![],
        };

        indexer.run_incremental(&mut storage, &refresh_info, &bus, None)?;

        let mut saw_warning = false;
        for _ in 0..32 {
            match rx.recv_timeout(Duration::from_millis(25)) {
                Ok(Event::ShowWarning { message }) => {
                    if message.contains("compile_commands.json") {
                        saw_warning = true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }

        assert!(
            saw_warning,
            "expected compile_commands warning event when loading fails"
        );
        Ok(())
    }

    #[test]
    fn test_node_kind_mapping_preserves_method_and_field() {
        assert_eq!(node_kind_from_graph_kind("METHOD"), NodeKind::METHOD);
        assert_eq!(node_kind_from_graph_kind("FIELD"), NodeKind::FIELD);
        assert_eq!(node_kind_from_graph_kind("INTERFACE"), NodeKind::INTERFACE);
    }

    #[test]
    fn test_header_language_defaults_to_c_without_compilation_metadata() {
        let config = get_language_for_ext("h").expect("header extension should resolve");
        assert_eq!(config.language_name, "c");
    }

    #[test]
    fn test_header_language_uses_cpp_when_compilation_standard_is_cxx() {
        let info = compilation_database::CompilationInfo {
            standard: Some(compilation_database::CxxStandard::Cxx20),
            ..Default::default()
        };
        let config =
            get_language_config_for_path(Path::new("widget.h"), Some(&info)).expect("config");
        assert_eq!(config.language_name, "cpp");
    }

    #[test]
    fn test_live_rule_registry_uses_split_rule_assets() {
        let rust = get_language_for_ext("rs").expect("rust config");
        assert_eq!(rust.graph_query, RUST_GRAPH_QUERY);
        assert_eq!(rust.tags_query, Some(RUST_TAGS_QUERY));

        let ts = get_language_for_ext("ts").expect("ts config");
        assert_eq!(ts.graph_query, TYPESCRIPT_GRAPH_QUERY);
        assert_eq!(ts.tags_query, Some(TYPESCRIPT_TAGS_QUERY));

        let tsx = get_language_for_ext("tsx").expect("tsx config");
        assert_eq!(tsx.graph_query, TSX_GRAPH_QUERY);
        assert_eq!(tsx.tags_query, Some(TSX_TAGS_QUERY));
    }

    #[test]
    fn test_compiled_rules_cache_reuses_compiled_artifacts() -> Result<()> {
        let config = get_language_for_ext("tsx").expect("tsx config");
        let first = config.compiled_rules()? as *const CompiledLanguageRules;
        let second = config.compiled_rules()? as *const CompiledLanguageRules;
        assert_eq!(first, second, "compiled rules should be cached per language");
        Ok(())
    }

    #[test]
    fn test_raw_graph_contracts_cover_supported_languages() -> Result<()> {
        let python = execute_raw_graph_contract(
            Path::new("sample.py"),
            r#"
from app.helpers import tool

class Worker:
    def run(self):
        tool()
"#,
            &get_language_for_ext("py").expect("python config"),
        )?;
        assert!(python.nodes.contains(&("CLASS".to_string(), "Worker".to_string())));
        assert!(python.edges.contains(&(
            "Worker".to_string(),
            "run".to_string(),
            "MEMBER".to_string()
        )));

        let java = execute_raw_graph_contract(
            Path::new("Sample.java"),
            r#"
class Base {}
class Child extends Base {
    void run() {}
}
"#,
            &get_language_for_ext("java").expect("java config"),
        )?;
        assert!(java.edges.contains(&(
            "Child".to_string(),
            "Base".to_string(),
            "INHERITANCE".to_string()
        )));

        let rust = execute_raw_graph_contract(
            Path::new("main.rs"),
            r#"
use crate::helpers::tool;

struct Worker;

impl Worker {
    fn run(&self) {
        tool::<u32>();
    }
}
"#,
            &get_language_for_ext("rs").expect("rust config"),
        )?;
        assert!(rust.nodes.contains(&("STRUCT".to_string(), "Worker".to_string())));
        assert!(rust.edges.contains(&(
            "crate::helpers::tool".to_string(),
            "crate::helpers::tool".to_string(),
            "IMPORT".to_string()
        )));

        let javascript = execute_raw_graph_contract(
            Path::new("main.js"),
            r#"
import thing from "./dep";

function run() {
    thing();
}
"#,
            &get_language_for_ext("js").expect("javascript config"),
        )?;
        assert!(javascript.edges.contains(&(
            "\"./dep\"".to_string(),
            "\"./dep\"".to_string(),
            "IMPORT".to_string()
        )));
        assert!(javascript.edges.contains(&(
            "thing".to_string(),
            "thing".to_string(),
            "CALL".to_string()
        )));

        let typescript = execute_raw_graph_contract(
            Path::new("main.ts"),
            r#"
interface Base {}
interface Child extends Base {}
"#,
            &get_language_for_ext("ts").expect("typescript config"),
        )?;
        assert!(typescript.edges.contains(&(
            "Child".to_string(),
            "Base".to_string(),
            "INHERITANCE".to_string()
        )));

        let tsx = execute_raw_graph_contract(
            Path::new("main.tsx"),
            r#"
type Props = { label: string };

function Badge(props: Props) {
    return <span>{props.label}</span>;
}

class View {
    render() {
        return <Badge label="hi" />;
    }
}
"#,
            &get_language_for_ext("tsx").expect("tsx config"),
        )?;
        assert!(tsx.edges.contains(&(
            "render".to_string(),
            "Badge".to_string(),
            "USAGE".to_string()
        )));
        assert!(tsx.edges.contains(&(
            "render".to_string(),
            "label".to_string(),
            "USAGE".to_string()
        )));

        let cpp = execute_raw_graph_contract(
            Path::new("main.cpp"),
            r#"
struct Base {};

template <typename T>
struct Wrapper {};

struct Child : Base {
    Wrapper<int> value;
};
"#,
            &get_language_for_ext("cpp").expect("cpp config"),
        )?;
        assert!(cpp.edges.contains(&(
            "Child".to_string(),
            "Base".to_string(),
            "INHERITANCE".to_string()
        )));

        let c = execute_raw_graph_contract(
            Path::new("main.h"),
            r#"
typedef struct Worker {
    int value;
} Worker;
"#,
            &get_language_for_ext("h").expect("c config"),
        )?;
        assert!(c.edges.contains(&(
            "Worker".to_string(),
            "value".to_string(),
            "MEMBER".to_string()
        )));

        Ok(())
    }

    #[test]
    fn test_live_rule_parsers_expose_key_node_kinds() {
        let python_kinds = parser_node_kinds(tree_sitter_python::LANGUAGE.into());
        for kind in ["class_definition", "function_definition", "call"] {
            assert!(
                python_kinds.contains(kind),
                "python grammar should expose {kind}"
            );
        }

        let java_kinds = parser_node_kinds(tree_sitter_java::LANGUAGE.into());
        for kind in ["class_declaration", "method_declaration", "method_invocation"] {
            assert!(
                java_kinds.contains(kind),
                "java grammar should expose {kind}"
            );
        }

        let rust_kinds = parser_node_kinds(tree_sitter_rust::LANGUAGE.into());
        for kind in ["struct_item", "impl_item", "call_expression", "use_declaration"] {
            assert!(
                rust_kinds.contains(kind),
                "rust grammar should expose {kind}"
            );
        }

        let js_kinds = parser_node_kinds(tree_sitter_javascript::LANGUAGE.into());
        for kind in [
            "function_declaration",
            "call_expression",
            "import_statement",
        ] {
            assert!(
                js_kinds.contains(kind),
                "javascript grammar should expose {kind}"
            );
        }

        let ts_kinds = parser_node_kinds(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into());
        for kind in [
            "interface_declaration",
            "class_declaration",
            "method_definition",
            "generic_type",
        ] {
            assert!(
                ts_kinds.contains(kind),
                "typescript grammar should expose {kind}"
            );
        }

        let tsx_kinds = parser_node_kinds(tree_sitter_typescript::LANGUAGE_TSX.into());
        for kind in [
            "jsx_element",
            "jsx_self_closing_element",
            "jsx_expression",
            "jsx_attribute",
        ] {
            assert!(tsx_kinds.contains(kind), "tsx grammar should expose {kind}");
        }

        let cpp_kinds = parser_node_kinds(tree_sitter_cpp::LANGUAGE.into());
        for kind in ["template_type", "field_declaration", "class_specifier"] {
            assert!(cpp_kinds.contains(kind), "cpp grammar should expose {kind}");
        }

        let c_kinds = parser_node_kinds(tree_sitter_c::LANGUAGE.into());
        for kind in ["struct_specifier", "field_declaration", "type_definition"] {
            assert!(c_kinds.contains(kind), "c grammar should expose {kind}");
        }
    }

    #[test]
    fn test_cpp_template_type_arguments_support_multiline_and_nested_templates() -> Result<()> {
        let cpp_code = r#"
struct Key {};
struct Value {};

template <typename T>
struct Wrapper {};

template <typename T, typename U>
struct PairStore {};

struct Holder {
    PairStore<
        Key,
        Wrapper<Value> // keep nested templates and comments parse-driven
    > store;
};
"#;
        let language_config = get_language_for_ext("cpp").expect("cpp config");
        let result = index_file(
            Path::new("holder.cpp"),
            cpp_code,
            &language_config,
            None,
            None,
        )?;

        let node_by_id = result
            .nodes
            .iter()
            .map(|node| (node.id, node))
            .collect::<HashMap<_, _>>();
        let has_type_argument = |source_suffix: &str, target_suffix: &str| {
            result.edges.iter().any(|edge| {
                edge.kind == EdgeKind::TYPE_ARGUMENT
                    && node_by_id
                        .get(&edge.source)
                        .is_some_and(|node| node.serialized_name.ends_with(source_suffix))
                    && node_by_id
                        .get(&edge.target)
                        .is_some_and(|node| node.serialized_name.ends_with(target_suffix))
            })
        };

        assert!(
            has_type_argument("PairStore", "Key"),
            "expected PairStore -> Key type argument edge"
        );
        assert!(
            has_type_argument("PairStore", "Wrapper"),
            "expected PairStore -> Wrapper type argument edge"
        );

        Ok(())
    }

    #[test]
    fn test_incomplete_parse_marks_file_incomplete() -> Result<()> {
        let code = "fn broken( {\n";
        let language_config = get_language_for_ext("rs").unwrap();
        let result = index_file(Path::new("broken.rs"), code, &language_config, None, None)?;
        assert_eq!(result.files.len(), 1);
        assert!(
            !result.files[0].complete,
            "malformed syntax should mark the file incomplete"
        );
        Ok(())
    }
}
