use anyhow::{Result, anyhow};
use codestory_core::{Edge, EdgeId, EdgeKind, Node, NodeId, NodeKind, Occurrence, SourceLocation};
use codestory_storage::Storage;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use codestory_events::{Event, EventBus};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tree_sitter::{Language, Parser};
use tree_sitter_graph::ast::File as GraphFile;
use tree_sitter_graph::functions::Functions;
use tree_sitter_graph::{ExecutionConfig, NoCancellation, Variables};

pub mod cancellation;
pub mod compilation_database;
pub mod intermediate_storage;
pub mod resolution;
pub mod symbol_table;
pub use cancellation::CancellationToken;
use intermediate_storage::IntermediateStorage;
use symbol_table::SymbolTable;

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
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub occurrences: Vec<Occurrence>,
}

pub enum IndexingEvent {
    Progress(u64),
    Error(String),
    Finished,
}

pub struct WorkspaceIndexer {
    root: PathBuf,
    compilation_db: Option<compilation_database::CompilationDatabase>,
    batch_config: IncrementalIndexingConfig,
}

impl WorkspaceIndexer {
    pub fn new(root: PathBuf) -> Self {
        let compilation_db = compilation_database::CompilationDatabase::find_in_directory(&root)
            .and_then(|path| compilation_database::CompilationDatabase::load(path).ok());
        Self {
            root,
            compilation_db,
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
    ) -> Result<()> {
        event_bus.publish(Event::IndexingStarted {
            file_count: refresh_info.files_to_index.len(),
        });
        let total_files = refresh_info.files_to_index.len();
        let processed_count = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let root = self.root.clone();

        let symbol_table = Arc::new(SymbolTable::new());
        Self::seed_symbol_table(storage, &symbol_table);

        // Clone for parallel closure
        let cancelled_clone = cancelled.clone();
        if Self::is_cancelled(cancel_token) {
            return Ok(());
        }

        // 1. Parallel Indexing (chunked and flushed)
        let mut batched_storage = IntermediateStorage::default();
        let mut all_errors = Vec::new();
        let mut had_edges = false;
        let file_batch_size = self.batch_config.file_batch_size.max(1);
        let batch_config = self.batch_config;

        for file_chunk in refresh_info.files_to_index.chunks(file_batch_size) {
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

            for mut local_storage in chunk_results {
                all_errors.append(&mut local_storage.errors);
                batched_storage.merge(local_storage);

                let should_flush = !batched_storage.nodes.is_empty()
                    || !batched_storage.edges.is_empty()
                    || !batched_storage.occurrences.is_empty();
                if should_flush
                    && (batched_storage.nodes.len() >= batch_config.node_batch_size
                        || batched_storage.edges.len() >= batch_config.edge_batch_size
                        || batched_storage.occurrences.len() >= batch_config.occurrence_batch_size)
                {
                    Self::flush_projection_batch(storage, &mut batched_storage, &mut had_edges)?;
                }

                if all_errors.len() >= batch_config.error_batch_size {
                    Self::flush_errors(storage, &mut all_errors, batch_config.error_batch_size)?;
                }
            }

            if cancelled.load(Ordering::Relaxed) {
                event_bus.publish(Event::IndexingComplete { duration_ms: 0 });
                return Ok(());
            }
        }

        // Check if cancelled during indexing
        if cancelled.load(Ordering::Relaxed) {
            event_bus.publish(Event::IndexingComplete { duration_ms: 0 });
            return Ok(());
        }

        Self::flush_projection_batch(storage, &mut batched_storage, &mut had_edges)?;

        // 3.5 Resolve call/import edges post-pass
        if had_edges {
            let resolver = resolution::ResolutionPass::new();
            resolver
                .run(storage)
                .map_err(|e| anyhow!("Resolution error: {:?}", e))?;
        }

        // Write errors
        while !all_errors.is_empty() {
            Self::flush_errors(storage, &mut all_errors, batch_config.error_batch_size)?;
        }

        // 4. Cleanup removed files
        if !refresh_info.files_to_remove.is_empty() {
            storage
                .delete_files_batch(&refresh_info.files_to_remove)
                .map_err(|e| anyhow!("Storage cleanup error: {:?}", e))?;
        }

        event_bus.publish(Event::IndexingComplete { duration_ms: 0 });
        Ok(())
    }

    fn is_cancelled(cancel_token: Option<&CancellationToken>) -> bool {
        cancel_token.map(CancellationToken::is_cancelled).unwrap_or(false)
    }

    fn seed_symbol_table(storage: &Storage, symbol_table: &SymbolTable) {
        if let Ok(nodes) = storage.get_nodes() {
            for node in nodes {
                symbol_table.insert(node.id.0, node.kind);
            }
        }
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

        batched_storage.clear();
        Ok(())
    }

    fn index_path(
        &self,
        path: &PathBuf,
        root: &Path,
        symbol_table: &Arc<SymbolTable>,
    ) -> IntermediateStorage {
        let full_path = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };

        let mut local_storage = IntermediateStorage::default();
        let Some((lang, lang_name, graph_query)) =
            get_language_for_ext(full_path.extension().and_then(|s| s.to_str()).unwrap_or(""))
        else {
            return local_storage;
        };

        let compilation_info = self
            .compilation_db
            .as_ref()
            .and_then(|db| db.get_parsed_info(&full_path));

        match std::fs::read_to_string(&full_path) {
            Ok(source) => match index_file(
                &full_path,
                &source,
                lang,
                lang_name,
                graph_query,
                compilation_info,
                Some(Arc::clone(symbol_table)),
            ) {
                Ok(index_result) => {
                    local_storage.nodes = index_result.nodes;
                    local_storage.edges = index_result.edges;
                    local_storage.occurrences = index_result.occurrences;
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
            },
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

fn file_node_from_source(path: &Path, source: &str) -> (Node, String, NodeId) {
    let file_name = path.to_string_lossy().to_string();
    let file_id = NodeId(generate_id(&file_name));
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
        "FUNCTION" | "METHOD" => NodeKind::FUNCTION,
        "CLASS" | "STRUCT" => NodeKind::CLASS,
        "MODULE" | "NAMESPACE" => NodeKind::MODULE,
        "FILE" => NodeKind::FILE,
        "VARIABLE" | "FIELD" => NodeKind::VARIABLE,
        "CONSTANT" => NodeKind::CONSTANT,
        _ => NodeKind::UNKNOWN,
    }
}

fn definition_occurrences(unique_nodes: &HashMap<NodeId, Node>, file_id: NodeId) -> Vec<Occurrence> {
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

fn canonicalize_nodes(
    file_name: &str,
    final_nodes: Vec<Node>,
) -> (Vec<Node>, HashMap<NodeId, NodeId>) {
    let is_type_like = |kind: NodeKind| {
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
    };

    let mut id_remap: HashMap<NodeId, NodeId> = HashMap::new();
    let mut canonical_owner: HashMap<String, NodeId> = HashMap::new();
    let mut deduped_nodes = Vec::with_capacity(final_nodes.len());

    for mut node in final_nodes {
        let qualified_name = node.serialized_name.clone();
        node.qualified_name = Some(qualified_name.clone());

        let canonical_id = if is_type_like(node.kind) {
            format!("{}:{}", file_name, qualified_name)
        } else {
            let start_line = node.start_line.unwrap_or(1);
            format!("{}:{}:{}", file_name, qualified_name, start_line)
        };

        if is_type_like(node.kind)
            && let Some(existing_id) = canonical_owner.get(&canonical_id)
        {
            id_remap.insert(node.id, *existing_id);
            continue;
        }

        let new_id = NodeId(generate_id(&canonical_id));
        node.canonical_id = Some(canonical_id.clone());
        id_remap.insert(node.id, new_id);
        node.id = new_id;
        canonical_owner.insert(canonical_id, new_id);
        deduped_nodes.push(node);
    }

    (deduped_nodes, id_remap)
}

fn remap_file_affinity(nodes: &mut [Node], new_file_id: NodeId) {
    for node in nodes.iter_mut() {
        node.file_node_id = Some(new_file_id);
    }
}

fn remap_edges(edges: &mut [Edge], new_file_id: NodeId, id_remap: &HashMap<NodeId, NodeId>) {
    for edge in edges.iter_mut() {
        if let Some(new_id) = id_remap.get(&edge.source) {
            edge.source = *new_id;
        }
        if let Some(new_id) = id_remap.get(&edge.target) {
            edge.target = *new_id;
        }
        edge.file_node_id = Some(new_file_id);
        edge.id = EdgeId(generate_edge_id(edge.source.0, edge.target.0, edge.kind));
    }
}

fn remap_occurrences(
    occurrences: &mut [Occurrence],
    id_remap: &HashMap<NodeId, NodeId>,
) {
    for occ in occurrences.iter_mut() {
        if let Some(new_id) = id_remap.get(&NodeId(occ.element_id)) {
            occ.element_id = new_id.0;
        }
        if let Some(new_file_id) = id_remap.get(&occ.location.file_node_id) {
            occ.location.file_node_id = *new_file_id;
        }
    }
}

/// Index a file and return the results.
pub fn index_file(
    path: &Path,
    source: &str,
    language: Language,
    language_name: &str,
    graph_query: &str,
    compilation_info: Option<compilation_database::CompilationInfo>,
    symbol_table: Option<Arc<SymbolTable>>,
) -> Result<IndexResult> {
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|e| anyhow!("Language error: {:?}", e))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse source"))?;

    let file = GraphFile::from_str(language.clone(), graph_query)
        .map_err(|e| anyhow!("Graph DSL error: {:?}", e))?;

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

    let graph = file
        .execute(&tree, source, &config, &NoCancellation)
        .map_err(|e| anyhow!("Graph execution error: {:?}", e))?;

    let mut result_nodes = Vec::new();
    let mut result_edges = Vec::new();
    let mut result_occurrences = Vec::new();

    // 0. Create file node.
    let (file_node, file_name, file_id) = file_node_from_source(path, source);
    result_nodes.push(file_node);

    // 1. First pass: Create nodes and a temporary mapping from GraphNodeId -> OurNodeId
    let mut graph_to_node_id = HashMap::new();
    let mut unique_nodes: HashMap<NodeId, Node> = HashMap::new();

    for node_id in graph.iter_nodes() {
        let node_data = &graph[node_id];

        let mut kind_str = String::new();
        let mut name_str = String::new();
        let mut start_row: Option<u32> = None;
        let mut start_col: Option<u32> = None;
        let mut end_row: Option<u32> = None;
        let mut end_col: Option<u32> = None;

        for (attr, val) in node_data.attributes.iter() {
            if attr.as_str() == "kind" {
                kind_str = val.as_str().unwrap_or("UNKNOWN").to_string();
            }
            if attr.as_str() == "name" {
                name_str = val.as_str().unwrap_or("").to_string();
            }
            if attr.as_str() == "start_row" {
                start_row = val.as_integer().ok();
            }
            if attr.as_str() == "start_col" {
                start_col = val.as_integer().ok();
            }
            if attr.as_str() == "end_row" {
                end_row = val.as_integer().ok();
            }
            if attr.as_str() == "end_col" {
                end_col = val.as_integer().ok();
            }
        }

        if !kind_str.is_empty() && !name_str.is_empty() {
            let kind = node_kind_from_graph_kind(kind_str.as_str());

            let start_line = start_row.map(|v| v + 1).unwrap_or(1);
            let canonical_seed = format!("{}:{}:{}", file_name, name_str, start_line);
            let nid = NodeId(generate_id(&canonical_seed));
            graph_to_node_id.insert(node_id, nid);

            unique_nodes.insert(
                nid,
                Node {
                    id: nid,
                    kind,
                    serialized_name: name_str,
                    start_line: Some(start_line),
                    start_col: start_col.map(|v| v + 1),
                    end_line: end_row.map(|v| v + 1),
                    end_col: end_col.map(|v| v + 1),
                    ..Default::default()
                },
            );

            if let Some(st) = &symbol_table {
                st.insert(nid.0, kind);
            }
        }
    }

    if !unique_nodes.is_empty() {
        result_nodes.extend(unique_nodes.values().cloned());
    }

    // 2. Second pass: Create edges using tree-sitter-graph output
    let mut edge_keys: HashSet<(NodeId, NodeId, EdgeKind)> = HashSet::new();

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
                    _ => {}
                }
            }

            let Some(kind) = kind else {
                continue;
            };

            if !edge_keys.insert((*source_id, *target_id, kind)) {
                continue;
            }

            let edge_pk = generate_edge_id(source_id.0, target_id.0, kind);
            result_edges.push(Edge {
                id: EdgeId(edge_pk),
                source: *source_id,
                target: *target_id,
                kind,
                file_node_id: Some(file_id),
                line,
                ..Default::default()
            });
        }
    }

    result_occurrences.extend(definition_occurrences(&unique_nodes, file_id));

    // 3. Resolve qualified names, canonicalize IDs, and remap projections.
    let final_nodes = apply_qualified_names(result_nodes, &result_edges, language_name);
    let (mut final_nodes, id_remap) = canonicalize_nodes(&file_name, final_nodes);
    let new_file_id = id_remap.get(&file_id).copied().unwrap_or(file_id);
    remap_file_affinity(&mut final_nodes, new_file_id);
    remap_edges(&mut result_edges, new_file_id, &id_remap);
    remap_occurrences(&mut result_occurrences, &id_remap);

    apply_line_range_call_attribution(&final_nodes, &mut result_edges);

    if let Some(st) = &symbol_table {
        for node in &final_nodes {
            st.insert(node.id.0, node.kind);
        }
    }

    Ok(IndexResult {
        nodes: final_nodes,
        edges: result_edges,
        occurrences: result_occurrences,
    })
}

pub fn get_language_for_ext(ext: &str) -> Option<(Language, &'static str, &'static str)> {
    match ext {
        "py" => Some((
            tree_sitter_python::LANGUAGE.into(),
            "python",
            include_str!("../rules/python.scm"),
        )),
        "java" => Some((
            tree_sitter_java::LANGUAGE.into(),
            "java",
            include_str!("../rules/java.scm"),
        )),
        "rs" => Some((
            tree_sitter_rust::LANGUAGE.into(),
            "rust",
            include_str!("../rules/rust.scm"),
        )),
        "js" => Some((
            tree_sitter_javascript::LANGUAGE.into(),
            "javascript",
            include_str!("../rules/javascript.scm"),
        )),
        "ts" | "tsx" => Some((
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "typescript",
            include_str!("../rules/typescript.scm"),
        )),
        "cpp" | "cc" | "cxx" | "h" | "hpp" => Some((
            tree_sitter_cpp::LANGUAGE.into(),
            "cpp",
            include_str!("../rules/cpp.scm"),
        )),
        "c" => Some((
            tree_sitter_c::LANGUAGE.into(),
            "cpp",
            include_str!("../rules/c.scm"),
        )),
        _ => None,
    }
}

pub fn generate_id(name: &str) -> i64 {
    let mut h: u64 = 0x811c9dc5;
    for b in name.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x01000193);
    }
    h as i64
}

#[derive(Clone, Copy, Debug)]
struct FunctionRange {
    id: NodeId,
    start: u32,
    end: u32,
}

fn apply_line_range_call_attribution(nodes: &[Node], edges: &mut Vec<Edge>) {
    let mut functions_by_file: HashMap<NodeId, Vec<FunctionRange>> = HashMap::new();

    for node in nodes {
        if !matches!(
            node.kind,
            NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
        ) {
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

    let mut dedup: HashSet<(NodeId, NodeId, EdgeKind)> = HashSet::new();
    let mut updated_edges = Vec::with_capacity(edges.len());

    for edge in edges.iter_mut() {
        if edge.kind == EdgeKind::CALL {
            if let (Some(file_id), Some(line)) = (edge.file_node_id, edge.line)
                && let Some(ranges) = functions_by_file.get(&file_id)
                && let Some(best) = ranges
                    .iter()
                    .filter(|range| line >= range.start && line <= range.end)
                    .min_by_key(|range| (range.end - range.start, range.start))
            {
                edge.source = best.id;
            }
            edge.id = EdgeId(generate_edge_id(edge.source.0, edge.target.0, edge.kind));
        }

        let key = (edge.source, edge.target, edge.kind);
        if dedup.insert(key) {
            updated_edges.push(edge.clone());
        }
    }

    *edges = updated_edges;
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
    let mut h: u64 = 0x811c9dc5;
    let mut update = |val: i64| {
        for b in val.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x01000193);
        }
    };
    update(source);
    update(target);
    update(kind as i64);
    h as i64
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (lang, lang_name, graph_query) = get_language_for_ext("py").unwrap();

        let result = index_file(
            Path::new("test.py"),
            python_code,
            lang,
            lang_name,
            graph_query,
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
        let (lang, lang_name, graph_query) = get_language_for_ext("java").unwrap();

        let result = index_file(
            Path::new("Test.java"),
            java_code,
            lang,
            lang_name,
            graph_query,
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
        let (lang, lang_name, graph_query) = get_language_for_ext("rs").unwrap();

        let result = index_file(
            Path::new("main.rs"),
            rust_code,
            lang,
            lang_name,
            graph_query,
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
    fn test_index_cpp_semantics() -> Result<()> {
        let cpp_code = r#"
class MyClass {
    void myMethod() {}
};
"#;
        let (lang, lang_name, graph_query) = get_language_for_ext("cpp").unwrap();

        let result = index_file(
            Path::new("test.cpp"),
            cpp_code,
            lang,
            lang_name,
            graph_query,
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
        let (lang, lang_name, graph_query) = get_language_for_ext("ts").unwrap();

        let result = index_file(
            Path::new("test.ts"),
            ts_code,
            lang,
            lang_name,
            graph_query,
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
    fn test_incremental_indexing() -> Result<()> {
        use codestory_project::RefreshInfo;
        use codestory_storage::Storage;
        use std::fs;
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
                .any(|n| n.serialized_name == "Foo" && n.kind == NodeKind::CLASS)
        );
        assert!(
            nodes
                .iter()
                .any(|n| n.serialized_name == "bar" && n.kind == NodeKind::FUNCTION)
        );

        // Check progress events
        let events: Vec<Event> = rx.try_iter().collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::IndexingStarted { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::IndexingComplete { .. }))
        );

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
    fn test_index_cpp_advanced() -> Result<()> {
        let code = r#"
class Base {};
class Derived : public Base {
    int x;
    void foo() {}
};
"#;
        let (lang, lang_name, graph_query) = get_language_for_ext("cpp").unwrap();
        let result = index_file(
            Path::new("test.cpp"),
            code,
            lang,
            lang_name,
            graph_query,
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
        let (lang, lang_name, graph_query) = get_language_for_ext("py").unwrap();
        let result = index_file(
            Path::new("test.py"),
            code,
            lang,
            lang_name,
            graph_query,
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
        let (lang, lang_name, graph_query) = get_language_for_ext("rs").unwrap();
        let result = index_file(
            Path::new("main.rs"),
            code,
            lang,
            lang_name,
            graph_query,
            None,
            None,
        )?;

        // Verify Trait Node
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name == "MyTrait" && n.kind == NodeKind::CLASS)
        );
        // Verify Impl Inheritance
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::INHERITANCE));
        // Verify Macro usage
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::USAGE));
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
        let (lang, lang_name, graph_query) = get_language_for_ext("java").unwrap();
        let result = index_file(
            Path::new("Test.java"),
            java_code,
            lang,
            lang_name,
            graph_query,
            None,
            None,
        )?;

        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name.ends_with(".caller") && n.kind == NodeKind::FUNCTION),
            "Caller node not found"
        );
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.serialized_name.ends_with(".callee") && n.kind == NodeKind::FUNCTION),
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
        let (lang, lang_name, graph_query) = get_language_for_ext("java").unwrap();
        let result = index_file(
            Path::new("Test.java"),
            java_code,
            lang,
            lang_name,
            graph_query,
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
}
