//! Parser-backed and structural indexing pipeline.
//!
//! The indexer turns source files selected by `codestory-workspace` into graph
//! projections persisted by `codestory-store`. Parser-backed languages use
//! tree-sitter graph rules and optional semantic resolution. Structural
//! collectors emit exact source anchors for files such as HTML, CSS, SQL,
//! GitHub Actions workflows, Docker Compose files, and Cargo manifests; those
//! anchors are source proof, not parser-backed language coverage.
//!
//! Freshness is owned by the caller's refresh plan. This crate assumes each
//! `index_file` or `WorkspaceIndexer::run` input is scheduled for indexing and
//! returns projection rows for storage to flush.

use anyhow::{Result, anyhow};
use codestory_contracts::graph::{
    AccessKind, CallableProjectionState, Edge, EdgeId, EdgeKind, FileCoverageReason, Node, NodeId,
    NodeKind, Occurrence, OccurrenceKind, ResolutionCertainty, SourceLocation,
};
use codestory_contracts::language_support::normalize_extension;
use codestory_contracts::workspace::{OversizedSourceExclusionCandidate, SourceIndexPolicy};
use codestory_store::{
    IndexArtifactCacheReader, IndexArtifactCacheWrite, StorageError, Store as Storage,
};
use crossbeam_channel::{Receiver, SendTimeoutError, bounded};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use codestory_contracts::events::{Event, EventBus};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node as TsNode, Parser, Point, Query, QueryCursor, Tree};
use tree_sitter_graph::ast::File as GraphFile;
use tree_sitter_graph::functions::Functions;
use tree_sitter_graph::{ExecutionConfig, NoCancellation, Variables};

#[cfg(test)]
thread_local! {
    static MANUAL_RECEIVER_LOOKUP_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static GO_METHOD_IDENTITY_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[inline]
fn count_manual_receiver_lookup_work(amount: usize) {
    #[cfg(test)]
    MANUAL_RECEIVER_LOOKUP_WORK.with(|work| work.set(work.get().saturating_add(amount)));
    #[cfg(not(test))]
    let _ = amount;
}

#[cfg(test)]
fn reset_manual_receiver_lookup_work() {
    MANUAL_RECEIVER_LOOKUP_WORK.with(|work| work.set(0));
}

#[cfg(test)]
fn manual_receiver_lookup_work() -> usize {
    MANUAL_RECEIVER_LOOKUP_WORK.with(std::cell::Cell::get)
}

#[inline]
fn count_go_method_identity_work(amount: usize) {
    #[cfg(test)]
    GO_METHOD_IDENTITY_WORK.with(|work| work.set(work.get().saturating_add(amount)));
    #[cfg(not(test))]
    let _ = amount;
}

#[cfg(test)]
fn reset_go_method_identity_work() {
    GO_METHOD_IDENTITY_WORK.with(|work| work.set(0));
}

#[cfg(test)]
fn go_method_identity_work() -> usize {
    GO_METHOD_IDENTITY_WORK.with(std::cell::Cell::get)
}

mod cache;
pub mod cancellation;
pub mod compilation_database;
mod framework_routes;
pub mod intermediate_storage;
mod language_configs;
mod languages;
mod proof_resolution;

/// SRC-C2 fence classification: lives in its own file because
/// `codestory-indexer`'s own source is indexed by
/// `tests/integration.rs`, which fails once a file crosses the 1 MB
/// oversized-source cap this crate enforces.
#[cfg(test)]
mod projection_fence_tests;
pub mod resolution;
pub mod semantic;
pub mod structural;
pub mod symbol_table;
pub mod template_pipeline;
use cache::{
    CachedIndexArtifact, CachedStructuralArtifact, build_index_artifact_cache_key,
    build_structural_artifact_cache_key, index_artifact_cache_path,
};
pub use cancellation::CancellationToken;
use intermediate_storage::IntermediateStorage;
#[cfg(debug_assertions)]
pub use proof_resolution::{BashResolutionWork, bash_resolution_work, reset_bash_resolution_work};
pub use proof_resolution::{
    build_funnel as build_proof_resolution_funnel, current_proof_resolution_adapter_roster,
    rematerialize_proof_resolution_projection,
};
use symbol_table::SymbolTable;

pub(crate) const RECEIVER_OWNER_CALLSITE_PREFIX: &str = "receiver-owner:";
pub(crate) const RECEIVER_MODULE_CALLSITE_PREFIX: &str = "receiver-module:";
/// Prefix shared by all receiver-binding markers (e.g. the PHP foreach
/// element marker `receiver-binding:loop-element@{start}-{end}`). Binding
/// markers survive placeholder replacement: when an in-file resolution removes
/// a generic placeholder edge, its binding markers move to the resolved edge.
pub(crate) const RECEIVER_BINDING_CALLSITE_PREFIX: &str = "receiver-binding:";
/// Canonical-id prefix of import-resolved TYPE_USAGE reference nodes (P2a).
pub(crate) const TYPE_USAGE_REFERENCE_CANONICAL_PREFIX: &str = "type_reference:";
/// Canonical-id prefix of PENDING same-root TYPE_USAGE reference nodes. The
/// suffix is `{file}:{referencing_namespace}:{bare_name}`;
/// `finalize_pending_type_usage_edges` reads the fact back from it.
pub(crate) const TYPE_USAGE_PENDING_CANONICAL_PREFIX: &str = "type_ref_pending:";

#[derive(Debug, Clone, Copy)]
struct IndexFeatureFlags {
    legacy_edge_identity: bool,
    lazy_graph_execution: bool,
}

struct PostProcessedIndexResults {
    nodes: Vec<Node>,
    id_remap: HashMap<NodeId, NodeId>,
}

impl IndexFeatureFlags {
    fn from_env() -> Self {
        Self {
            legacy_edge_identity: env_flag("CODESTORY_INDEX_LEGACY_EDGE_IDENTITY", false)
                || env_flag("CODESTORY_INDEX_LEGACY_DEDUP", false),
            lazy_graph_execution: env_flag("CODESTORY_INDEX_GRAPH_LAZY", true),
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

fn parser_direct_structural_certainty(kind: EdgeKind) -> Option<ResolutionCertainty> {
    match kind {
        EdgeKind::MEMBER
        | EdgeKind::INHERITANCE
        | EdgeKind::OVERRIDE
        | EdgeKind::TYPE_ARGUMENT
        | EdgeKind::TEMPLATE_SPECIALIZATION
        | EdgeKind::INCLUDE
        | EdgeKind::IMPORT => Some(ResolutionCertainty::Certain),
        EdgeKind::CALL
        | EdgeKind::USAGE
        | EdgeKind::TYPE_USAGE
        | EdgeKind::MACRO_USAGE
        | EdgeKind::ANNOTATION_USAGE
        | EdgeKind::UNKNOWN => None,
    }
}

// Source of truth for live rule assets. Keep this registry aligned with
// `get_language_for_ext` so dead rule files do not silently linger.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LanguageRuleset {
    Python,
    Java,
    Rust,
    JavaScript,
    TypeScript,
    Tsx,
    Cpp,
    C,
    Go,
    Ruby,
    Php,
    CSharp,
    Kotlin,
    Swift,
    Dart,
    Bash,
}

/// Tree-sitter language plus graph/tag rules used for parser-backed indexing.
///
/// A `LanguageConfig` means CodeStory has parser rules for the file extension.
/// Structural collectors and text-only diagnostics are routed elsewhere and
/// should not be described as parser-backed graph support.
#[derive(Debug, Clone)]
pub struct LanguageConfig {
    pub language: Language,
    pub language_name: &'static str,
    pub graph_query: &'static str,
    pub tags_query: Option<&'static str>,
    ruleset: LanguageRuleset,
}

pub use codestory_contracts::language_support::{
    LanguageEvidenceTier, LanguageSupportMode, LanguageSupportProfile,
};

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
    fallback_index: HashMap<(String, u32), Vec<TagDefinitionKey>>,
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
                    .entry((key.name.clone(), key.start_line))
                    .or_default()
                    .push(key.clone());
                if let Some(keys) = self
                    .fallback_index
                    .get_mut(&(key.name.clone(), key.start_line))
                {
                    keys.sort_by_key(|key| key.start_col);
                    keys.dedup();
                }
                self.by_key.insert(key, definition);
            }
        }
    }

    fn take(
        &mut self,
        name: &str,
        start_line: u32,
        start_col: Option<u32>,
    ) -> Option<TagDefinition> {
        if let Some(start_col) = start_col {
            let exact_key = TagDefinitionKey {
                name: name.to_string(),
                start_line,
                start_col,
            };
            if let Some(definition) = self.by_key.remove(&exact_key) {
                self.remove_fallback_key(name, start_line, &exact_key);
                return Some(definition);
            }
        }

        let lookup = (name.to_string(), start_line);
        let fallback_key = {
            let keys = self.fallback_index.get_mut(&lookup)?;
            let index = start_col
                .and_then(|start_col| keys.iter().position(|key| key.start_col >= start_col))
                .unwrap_or(0);
            keys.remove(index)
        };
        if self.fallback_index.get(&lookup).is_some_and(Vec::is_empty) {
            self.fallback_index.remove(&lookup);
        }
        self.by_key.remove(&fallback_key)
    }

    fn remove_fallback_key(&mut self, name: &str, start_line: u32, key: &TagDefinitionKey) {
        let lookup = (name.to_string(), start_line);
        if let Some(keys) = self.fallback_index.get_mut(&lookup) {
            keys.retain(|candidate| candidate != key);
            if keys.is_empty() {
                self.fallback_index.remove(&lookup);
            }
        }
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
        // Registry first: a migrated language carries its rule file and cache in
        // `languages::<lang>`. The arms below are the not-yet-migrated residue.
        if let Some(extraction) = languages::extraction_for_ruleset(*self) {
            return compiled_rules_cache(
                language,
                extraction.graph_query,
                extraction.tags_query,
                extraction.compiled_rules,
            );
        }
        match self {
            // Answered by the registry above; these arms only exist because
            // the match must stay exhaustive. Failing closed here rather than
            // match must stay exhaustive. Failing closed here rather than
            // panicking keeps a future registry mistake a typed indexing error.
            LanguageRuleset::Python => Err(anyhow!(
                "python compiled rules are owned by the language registry"
            )),
            // Answered by the registry above; see the `Kotlin` arm below.
            LanguageRuleset::Java => Err(anyhow!(
                "java compiled rules are owned by the language registry"
            )),
            // Answered by the registry above; see the Kotlin arm below.
            LanguageRuleset::Rust => Err(anyhow!(
                "rust compiled rules are owned by the language registry"
            )),
            // Answered by the registry above; the arm only exists because the
            // match must stay exhaustive. Failing closed here rather than
            // panicking keeps a future registry mistake a typed indexing error.
            LanguageRuleset::JavaScript => Err(anyhow!(
                "javascript compiled rules are owned by the language registry"
            )),
            // Answered by the registry above; see the Kotlin arm below.
            LanguageRuleset::TypeScript => Err(anyhow!(
                "typescript compiled rules are owned by the language registry"
            )),
            // Answered by the registry above; see the `Kotlin` arm below.
            LanguageRuleset::Tsx => Err(anyhow!(
                "tsx compiled rules are owned by the language registry"
            )),
            // Answered by the registry above; see the `Kotlin` arm below.
            LanguageRuleset::Cpp => Err(anyhow!(
                "cpp compiled rules are owned by the language registry"
            )),
            LanguageRuleset::Go => Err(anyhow!(
                "go compiled rules are owned by the language registry"
            )),
            // Answered by the registry above; see the Kotlin arm below.
            LanguageRuleset::Ruby => Err(anyhow!(
                "ruby compiled rules are owned by the language registry"
            )),
            // Answered by the registry above; the arm only exists because the
            // match must stay exhaustive. Failing closed here rather than
            // panicking keeps a future registry mistake a typed indexing error.
            LanguageRuleset::Php => Err(anyhow!(
                "php compiled rules are owned by the language registry"
            )),
            // Answered by the registry above; see the `Kotlin` arm below.
            LanguageRuleset::CSharp => Err(anyhow!(
                "csharp compiled rules are owned by the language registry"
            )),
            // Answered by the registry above; these arms only exist because the
            // panicking keeps a future registry mistake a typed indexing error.
            LanguageRuleset::Kotlin => Err(anyhow!(
                "kotlin compiled rules are owned by the language registry"
            )),
            LanguageRuleset::C => Err(anyhow!(
                "c compiled rules are owned by the language registry"
            )),
            LanguageRuleset::Swift => Err(anyhow!(
                "swift compiled rules are owned by the language registry"
            )),
            LanguageRuleset::Dart => Err(anyhow!(
                "dart compiled rules are owned by the language registry"
            )),
            // Answered by the registry above; the arm only exists because the
            // match must stay exhaustive. Failing closed here rather than
            // panicking keeps a future registry mistake a typed indexing error.
            LanguageRuleset::Bash => Err(anyhow!(
                "bash compiled rules are owned by the language registry"
            )),
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
            .map(|query| {
                Query::new(&language, query).map_err(|e| format!("Tag query error: {:?}", e))
            })
            .transpose()?;
        Ok::<CompiledLanguageRules, String>(CompiledLanguageRules {
            graph_file,
            tags_query,
        })
    });

    compiled
        .as_ref()
        .map_err(|message| anyhow!(message.clone()))
}

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
        "macro" => Some(NodeKind::MACRO),
        "typedef" => Some(NodeKind::TYPEDEF),
        "union" => Some(NodeKind::UNION),
        "function" => Some(NodeKind::FUNCTION),
        "method" => Some(NodeKind::METHOD),
        "field" => Some(NodeKind::FIELD),
        "enum_constant" => Some(NodeKind::ENUM_CONSTANT),
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

    let mut matches = cursor.matches(query, tree.root_node(), source_bytes);
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
                .copied()
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
        cpp_language_config()
    } else {
        c_language_config()
    }
}

/// C++ config for the `.h` header seam.
///
/// `h` is routed to `c` by the public registry, so this path cannot go through
/// `get_language_for_ext`; it names the C++ registry row directly. The row is a
/// `const`, so there is nothing to fail closed on.
/// Parser config for C, built from its registry row.
///
/// The extension route reaches the same row through
/// `language_configs::get_language_for_ext`; this seam exists because a bare
/// `.h` is decided by compilation-database evidence rather than by extension.
fn c_language_config() -> LanguageConfig {
    let extraction = &languages::c::EXTRACTION;
    make_language_config(
        (extraction.parser_language)(),
        extraction.language_name,
        extraction.graph_query,
        extraction.tags_query,
        extraction.ruleset,
    )
}

fn cpp_language_config() -> LanguageConfig {
    let extraction = &languages::cpp::EXTRACTION;
    make_language_config(
        (extraction.parser_language)(),
        extraction.language_name,
        extraction.graph_query,
        extraction.tags_query,
        extraction.ruleset,
    )
}

fn path_is_c_header(path: &Path) -> bool {
    normalized_path_extension(path).as_deref() == Some("h")
}

pub(crate) fn normalized_path_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(normalize_extension)
}

fn header_source_has_cpp_signals(source: &str) -> bool {
    source
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with("//")
                && !line.starts_with("/*")
                && !line.starts_with('*')
        })
        .any(|line| {
            line.starts_with("class ")
                || line.starts_with("namespace ")
                || line.starts_with("template <")
                || line.starts_with("template<")
                || line == "public:"
                || line == "private:"
                || line == "protected:"
                || line.contains(" virtual ")
                || line.starts_with("virtual ")
                || line.contains("std::")
                || line.contains("::")
        })
}

fn maybe_upgrade_header_language_from_source(
    path: &Path,
    source: &str,
    language_config: &LanguageConfig,
) -> Option<LanguageConfig> {
    if language_config.language_name == "c"
        && path_is_c_header(path)
        && header_source_has_cpp_signals(source)
    {
        Some(cpp_language_config())
    } else {
        None
    }
}

fn get_language_config_for_path(
    path: &Path,
    compilation_info: Option<&compilation_database::CompilationInfo>,
) -> Option<LanguageConfig> {
    let ext = normalized_path_extension(path).unwrap_or_default();
    if ext == "h" {
        return Some(infer_header_language_config(compilation_info));
    }
    get_language_for_ext(&ext)
}

/// Batch sizes used while flushing incremental indexing output.
///
/// These values tune memory and write granularity only. They do not change
/// parser routing, graph semantics, or freshness decisions.
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

impl IncrementalIndexingConfig {
    fn for_mode(mode: codestory_workspace::BuildMode) -> Self {
        match mode {
            codestory_workspace::BuildMode::Incremental => Self::default(),
            codestory_workspace::BuildMode::FullRefresh => Self {
                // Full-refresh file scheduling uses the separate adaptive byte/node
                // budget below. This field remains the serial incremental window and
                // an explicit caller-provided full-refresh safety ceiling.
                file_batch_size: Self::default().file_batch_size,
                node_batch_size: 120_000,
                edge_batch_size: 120_000,
                occurrence_batch_size: 120_000,
                error_batch_size: 2_000,
            },
        }
    }
}

const DEFAULT_FULL_REFRESH_CHUNK_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_FULL_REFRESH_CHUNK_PROJECTED_NODES: usize = 120_000;
const DEFAULT_FULL_REFRESH_CHUNK_FILE_CEILING: usize = 512;

#[derive(Debug, Clone, Copy)]
struct FullRefreshChunkBudget {
    source_bytes: u64,
    projected_nodes: usize,
    file_ceiling: usize,
}

impl Default for FullRefreshChunkBudget {
    fn default() -> Self {
        Self {
            source_bytes: DEFAULT_FULL_REFRESH_CHUNK_SOURCE_BYTES,
            projected_nodes: DEFAULT_FULL_REFRESH_CHUNK_PROJECTED_NODES,
            file_ceiling: DEFAULT_FULL_REFRESH_CHUNK_FILE_CEILING,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct FullRefreshChunkPlan {
    start: usize,
    end: usize,
    source_bytes: u64,
    projected_nodes: usize,
}

struct AdaptiveFullRefreshChunkPlanner {
    budget: FullRefreshChunkBudget,
    last_source_bytes: u64,
    last_nodes: usize,
    #[cfg(test)]
    before_plan_file: Option<FullRefreshChunkTestHook>,
}

impl AdaptiveFullRefreshChunkPlanner {
    fn new(budget: FullRefreshChunkBudget) -> Self {
        Self {
            budget: FullRefreshChunkBudget {
                source_bytes: budget.source_bytes.max(1),
                projected_nodes: budget.projected_nodes.max(1),
                file_ceiling: budget.file_ceiling.max(1),
            },
            last_source_bytes: 0,
            last_nodes: 0,
            #[cfg(test)]
            before_plan_file: None,
        }
    }

    fn next_chunk(
        &self,
        files: &[PathBuf],
        root: &Path,
        start: usize,
        cancel_token: Option<&CancellationToken>,
    ) -> Option<FullRefreshChunkPlan> {
        if start >= files.len() {
            return None;
        }

        let mut source_bytes = 0u64;
        let mut projected_nodes = 0usize;
        let mut end = start;
        while end < files.len() && end - start < self.budget.file_ceiling {
            #[cfg(test)]
            if let Some(hook) = &self.before_plan_file {
                hook(end);
            }
            if cancel_token.is_some_and(CancellationToken::is_cancelled) {
                break;
            }
            let file_bytes =
                std::fs::metadata(WorkspaceIndexer::normalize_index_path(root, &files[end]))
                    .map(|metadata| metadata.len())
                    .unwrap_or(0);
            let file_projected_nodes = self.projected_nodes(file_bytes);
            let next_source_bytes = source_bytes.saturating_add(file_bytes);
            let next_projected_nodes = projected_nodes.saturating_add(file_projected_nodes);
            if end > start
                && (next_source_bytes > self.budget.source_bytes
                    || next_projected_nodes > self.budget.projected_nodes)
            {
                break;
            }
            source_bytes = next_source_bytes;
            projected_nodes = next_projected_nodes;
            end += 1;
        }

        if end == start {
            return None;
        }

        Some(FullRefreshChunkPlan {
            start,
            end,
            source_bytes,
            projected_nodes,
        })
    }

    #[cfg(test)]
    fn set_before_plan_file_hook(&mut self, hook: Option<FullRefreshChunkTestHook>) {
        self.before_plan_file = hook;
    }

    fn observe(&mut self, source_bytes: u64, nodes: usize) {
        self.last_source_bytes = source_bytes;
        self.last_nodes = nodes;
    }

    fn projected_nodes(&self, source_bytes: u64) -> usize {
        let (density_nodes, density_bytes) = if self.last_source_bytes == 0 {
            (self.budget.projected_nodes, self.budget.source_bytes)
        } else {
            (self.last_nodes.max(1), self.last_source_bytes)
        };
        let numerator = u128::from(source_bytes.max(1)).saturating_mul(density_nodes as u128);
        let projected = numerator.saturating_add(u128::from(density_bytes.saturating_sub(1)))
            / u128::from(density_bytes);
        usize::try_from(projected).unwrap_or(usize::MAX).max(1)
    }
}

/// In-memory graph projection for one indexed source.
///
/// Callers pass these vectors to `codestory-store` as one coherent projection
/// batch. `errors` are tracked separately in `IntermediateStorage` during
/// workspace runs.
pub struct IndexResult {
    pub files: Vec<codestory_store::FileInfo>,
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
    Delta {
        changed_callers: Vec<NodeId>,
    },
    /// The file-structural fence moved without changing what it contains.
    ///
    /// Every unowned row kept its identity and only its span shifted, so the
    /// rows the fence owns are deleted and re-inserted at their new positions
    /// while the node table — and everything anchored to it — is left alone.
    /// `changed_callers` is still repaired caller-scoped, exactly as in
    /// `Delta`.
    RepositionUnowned {
        changed_callers: Vec<NodeId>,
    },
    FullReplace,
}

/// Progress event emitted by low-level indexing flows.
pub enum IndexingEvent {
    Progress(u64),
    Error(String),
    Finished,
}

/// Storage access policy for one artifact-cache family.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ArtifactCachePolicy {
    KnownEmpty,
    #[default]
    ReadThrough,
}

impl ArtifactCachePolicy {
    fn reads_storage(self) -> bool {
        matches!(self, Self::ReadThrough)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArtifactCachePolicies {
    pub parser: ArtifactCachePolicy,
    pub structural: ArtifactCachePolicy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArtifactCacheFamilyStats {
    pub policy: ArtifactCachePolicy,
    pub logical_lookups: usize,
    pub physical_queries: usize,
    pub hits: usize,
    pub misses: usize,
    pub reader_opens: usize,
    pub lookup_wall_ns: u64,
}

impl ArtifactCacheFamilyStats {
    fn new(policy: ArtifactCachePolicy) -> Self {
        Self {
            policy,
            ..Self::default()
        }
    }

    fn record_lookup(&mut self) {
        self.logical_lookups = self.logical_lookups.saturating_add(1);
    }

    fn record_query(&mut self, elapsed: Duration) {
        self.physical_queries = self.physical_queries.saturating_add(1);
        self.lookup_wall_ns = self
            .lookup_wall_ns
            .saturating_add(elapsed.as_nanos().min(u64::MAX as u128) as u64);
    }
}

/// Timings and counters collected during a workspace indexing run.
#[derive(Debug, Clone, Copy, Default)]
pub struct IncrementalIndexingStats {
    /// Whether this run changed any graph-owned projection row rather than
    /// only refreshing verified file identity and diagnostics.
    pub graph_projection_changed: bool,
    /// Existing files whose parser projection was byte-for-byte stable while
    /// their source identity changed.
    pub source_identity_only_files: usize,
    pub setup_existing_projection_ids_ms: u64,
    pub setup_seed_symbol_table_ms: u64,
    /// Full source preparation wall time, including artifact-cache lookups.
    pub source_prepare_ms: u64,
    /// Backward-compatible alias for `source_prepare_ms`.
    pub artifact_cache_lookup_ms: u64,
    pub artifact_cache_write_ms: u64,
    pub artifact_cache_hits: usize,
    pub artifact_cache_misses: usize,
    pub artifact_cache_invalid_entries: usize,
    pub parser_artifact_cache: ArtifactCacheFamilyStats,
    pub structural_artifact_cache: ArtifactCacheFamilyStats,
    pub artifact_cache_writes: usize,
    pub artifact_cache_write_transactions: usize,
    pub full_refresh_chunks_produced: usize,
    pub full_refresh_chunks_persisted: usize,
    pub full_refresh_queue_capacity: usize,
    pub full_refresh_queue_high_water: usize,
    pub full_refresh_producer_blocked_ms: u64,
    pub full_refresh_writer_idle_ms: u64,
    pub full_refresh_chunk_target_bytes: u64,
    pub full_refresh_chunk_target_nodes: usize,
    pub full_refresh_chunk_file_ceiling: usize,
    pub full_refresh_chunk_max_files: usize,
    pub full_refresh_chunk_max_planned_bytes: u64,
    pub full_refresh_chunk_max_nodes: usize,
    pub full_refresh_chunk_budget_overruns: usize,
    pub full_refresh_chunk_planning_ms: u64,
    pub parse_index_ms: u64,
    pub projection_flush_ms: u64,
    pub projection_batch_wall_ms: u64,
    pub projection_batch_transactions: usize,
    pub projection_persistence: codestory_store::ProjectionPersistenceStats,
    pub flush_files_ms: u64,
    pub flush_nodes_ms: u64,
    pub flush_structural_text_units_ms: u64,
    pub flush_edges_ms: u64,
    pub flush_occurrences_ms: u64,
    pub flush_component_access_ms: u64,
    pub flush_callable_projection_ms: u64,
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
    pub resolution_override_count_ms: u64,
    pub resolution_calls_ms: u64,
    pub resolution_imports_ms: u64,
    pub resolution_cleanup_ms: u64,
    pub resolution_call_candidate_index_ms: u64,
    pub resolution_import_candidate_index_ms: u64,
    pub resolution_call_semantic_index_ms: u64,
    pub resolution_import_semantic_index_ms: u64,
    pub resolution_support_snapshot_load_ms: u64,
    pub resolution_support_snapshot_store_ms: u64,
    pub resolution_support_snapshot_hit: bool,
    pub resolution_support_snapshot_limit_bytes: u64,
    pub resolution_support_snapshot_stored: bool,
    pub resolution_support_snapshot_skipped_oversize: bool,
    pub resolution_call_semantic_candidates_ms: u64,
    pub resolution_import_semantic_candidates_ms: u64,
    pub resolution_call_semantic_requests: usize,
    pub resolution_call_semantic_unique_requests: usize,
    pub resolution_call_semantic_skipped_requests: usize,
    pub resolution_import_semantic_requests: usize,
    pub resolution_import_semantic_unique_requests: usize,
    pub resolution_import_semantic_skipped_requests: usize,
    pub resolution_call_compute_ms: u64,
    pub resolution_import_compute_ms: u64,
    pub resolution_call_apply_ms: u64,
    pub resolution_import_apply_ms: u64,
    pub resolution_override_resolution_ms: u64,
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

/// Indexing statistics plus verified bounded-source exclusions discovered by collectors.
#[derive(Debug, Clone)]
pub struct WorkspaceIndexingOutcome {
    pub stats: IncrementalIndexingStats,
    pub policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
}

#[derive(Debug)]
struct PreparedIndexInput {
    full_path: PathBuf,
    artifact_cache_path: Option<PathBuf>,
    source: String,
    source_utf8_exact: bool,
    compilation_info: Option<compilation_database::CompilationInfo>,
    language_config: LanguageConfig,
    artifact_cache_key: Option<String>,
    content_hash: String,
}

#[derive(Debug)]
struct PreparedStructuralInput {
    full_path: PathBuf,
    role_classification_path: PathBuf,
    artifact_cache_path: Option<PathBuf>,
    artifact_cache_key: Option<String>,
    source: String,
    content_hash: String,
}

enum PreparedIndexWork {
    Immediate(IntermediateStorage),
    Parse(PreparedIndexInput),
    Structural(PreparedStructuralInput),
}

enum PreparedIndexJob {
    Parse(Box<PreparedIndexInput>),
    Structural(PreparedStructuralInput),
}

struct PreparedIndexJobResult {
    local_storage: IntermediateStorage,
    cache_write: Option<ArtifactCacheWrite>,
    policy_exclusion: Option<PreparedPolicyExclusion>,
}

struct PreparedPolicyExclusion {
    file_id: i64,
    candidate: OversizedSourceExclusionCandidate,
}

struct ArtifactCacheWrite {
    path: PathBuf,
    cache_key: String,
    artifact_blob: Vec<u8>,
}

#[cfg(test)]
type FullRefreshChunkTestHook = Arc<dyn Fn(usize) + Send + Sync>;

enum ArtifactCacheBackend<'a> {
    Storage(&'a mut Storage),
    Reader(&'a IndexArtifactCacheReader),
    None,
    #[cfg(test)]
    FailReads,
}

struct ArtifactCacheAccess<'a> {
    backend: ArtifactCacheBackend<'a>,
    policies: ArtifactCachePolicies,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactCacheFamily {
    Parser,
    Structural,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FullRefreshCacheReadPlan {
    parser: bool,
    structural: bool,
    reader_owner: Option<ArtifactCacheFamily>,
}

impl<'a> ArtifactCacheAccess<'a> {
    fn storage(storage: &'a mut Storage, policies: ArtifactCachePolicies) -> Self {
        Self {
            backend: ArtifactCacheBackend::Storage(storage),
            policies,
        }
    }

    fn reader(
        reader: Option<&'a IndexArtifactCacheReader>,
        policies: ArtifactCachePolicies,
    ) -> Self {
        Self {
            backend: reader
                .map(ArtifactCacheBackend::Reader)
                .unwrap_or(ArtifactCacheBackend::None),
            policies,
        }
    }

    #[cfg(test)]
    fn failing(policies: ArtifactCachePolicies) -> Self {
        Self {
            backend: ArtifactCacheBackend::FailReads,
            policies,
        }
    }

    fn get_parser(
        &self,
        path: &Path,
        cache_key: &str,
        stats: &mut ArtifactCacheFamilyStats,
    ) -> std::result::Result<Option<Vec<u8>>, StorageError> {
        if !self.policies.parser.reads_storage() {
            return Ok(None);
        }
        let started = Instant::now();
        let result = match &self.backend {
            ArtifactCacheBackend::Storage(storage) => {
                storage.get_index_artifact_cache(path, cache_key)
            }
            ArtifactCacheBackend::Reader(reader) => reader.get(path, cache_key),
            ArtifactCacheBackend::None => {
                unreachable!("read-through parser cache access requires a storage connection")
            }
            #[cfg(test)]
            ArtifactCacheBackend::FailReads => Err(StorageError::Other(
                "injected parser cache read failure".into(),
            )),
        };
        stats.record_query(started.elapsed());
        result
    }

    fn get_structural(
        &self,
        path: &Path,
        cache_key: &str,
        stats: &mut ArtifactCacheFamilyStats,
    ) -> std::result::Result<Option<Vec<u8>>, StorageError> {
        if !self.policies.structural.reads_storage() {
            return Ok(None);
        }
        let started = Instant::now();
        let result = match &self.backend {
            ArtifactCacheBackend::Storage(storage) => {
                storage.get_structural_text_artifact_cache(path, cache_key)
            }
            ArtifactCacheBackend::Reader(reader) => reader.get_structural(path, cache_key),
            ArtifactCacheBackend::None => {
                unreachable!("read-through structural cache access requires a storage connection")
            }
            #[cfg(test)]
            ArtifactCacheBackend::FailReads => Err(StorageError::Other(
                "injected structural cache read failure".into(),
            )),
        };
        stats.record_query(started.elapsed());
        result
    }

    fn storage_mut(&mut self) -> Option<&mut Storage> {
        match &mut self.backend {
            ArtifactCacheBackend::Storage(storage) => Some(*storage),
            ArtifactCacheBackend::Reader(_) | ArtifactCacheBackend::None => None,
            #[cfg(test)]
            ArtifactCacheBackend::FailReads => None,
        }
    }
}

struct PreparedIndexChunk {
    cache_writes: Vec<ArtifactCacheWrite>,
    storages: Vec<IntermediateStorage>,
    policy_exclusions: Vec<PreparedPolicyExclusion>,
    progress_already_emitted: usize,
    #[cfg(test)]
    before_persist: Option<(usize, FullRefreshChunkTestHook)>,
}

impl PreparedIndexChunk {
    fn node_count(&self) -> usize {
        self.storages.iter().fold(0usize, |total, storage| {
            total.saturating_add(storage.nodes.len())
        })
    }
}

#[derive(Clone, Copy)]
struct IndexProgress<'a> {
    processed_count: &'a AtomicUsize,
    total_files: usize,
    event_bus: &'a EventBus,
}

impl IndexProgress<'_> {
    fn emit(self) {
        let current = self.processed_count.fetch_add(1, Ordering::Relaxed) + 1;
        self.event_bus.publish(Event::IndexingProgress {
            current,
            total: self.total_files,
        });
    }
}

#[cfg(test)]
#[derive(Clone, Default)]
struct FullRefreshPipelineTestHooks {
    before_plan_file: Option<FullRefreshChunkTestHook>,
    before_prepare_chunk: Option<FullRefreshChunkTestHook>,
    before_parse_job: Option<FullRefreshChunkTestHook>,
    before_writer_chunk: Option<FullRefreshChunkTestHook>,
    after_send_chunk: Option<FullRefreshChunkTestHook>,
    on_send_timeout: Option<FullRefreshChunkTestHook>,
}

struct ProjectionWriterOutput {
    stats: IncrementalIndexingStats,
    all_errors: Vec<codestory_contracts::graph::ErrorInfo>,
    had_edges: bool,
    policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
}

struct ProjectionWriter<'a> {
    storage: &'a mut Storage,
    mode: codestory_workspace::BuildMode,
    batch_config: IncrementalIndexingConfig,
    existing_projection_file_ids: &'a HashSet<i64>,
    replaced_projection_ids: HashSet<i64>,
    batched_storage: IntermediateStorage,
    batched_source_identity_only: bool,
    all_errors: Vec<codestory_contracts::graph::ErrorInfo>,
    pending_file_errors: Vec<codestory_contracts::graph::ErrorInfo>,
    fallback_file_error_ids: HashSet<i64>,
    fallback_file_errors: Vec<codestory_contracts::graph::ErrorInfo>,
    had_edges: bool,
    policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
    pipeline_telemetry: bool,
    stats: IncrementalIndexingStats,
}

impl<'a> ProjectionWriter<'a> {
    fn new(
        storage: &'a mut Storage,
        mode: codestory_workspace::BuildMode,
        batch_config: IncrementalIndexingConfig,
        existing_projection_file_ids: &'a HashSet<i64>,
        pipeline_telemetry: bool,
    ) -> Self {
        Self {
            storage,
            mode,
            batch_config,
            existing_projection_file_ids,
            replaced_projection_ids: HashSet::new(),
            batched_storage: IntermediateStorage::default(),
            batched_source_identity_only: true,
            all_errors: Vec::new(),
            pending_file_errors: Vec::new(),
            fallback_file_error_ids: HashSet::new(),
            fallback_file_errors: Vec::new(),
            had_edges: false,
            policy_exclusions: Vec::new(),
            pipeline_telemetry,
            stats: IncrementalIndexingStats::default(),
        }
    }

    fn storage_mut(&mut self) -> &mut Storage {
        self.storage
    }

    fn accept_chunk(
        &mut self,
        chunk: PreparedIndexChunk,
        progress: IndexProgress<'_>,
    ) -> Result<()> {
        #[cfg(test)]
        if let Some((chunk_index, hook)) = &chunk.before_persist {
            hook(*chunk_index);
        }
        if !chunk.cache_writes.is_empty() {
            let cache_write_started = Instant::now();
            let batch = chunk
                .cache_writes
                .iter()
                .map(|write| IndexArtifactCacheWrite {
                    path: &write.path,
                    cache_key: &write.cache_key,
                    artifact_blob: &write.artifact_blob,
                })
                .collect::<Vec<_>>();
            let written = self
                .storage
                .upsert_index_artifact_cache_batch(&batch)
                .map_err(|error| {
                    anyhow!(
                        "Storage cache batch write error for {} entries: {error}",
                        batch.len()
                    )
                })?;
            self.stats.artifact_cache_write_ms = self
                .stats
                .artifact_cache_write_ms
                .saturating_add(duration_ms_u64(cache_write_started.elapsed()));
            self.stats.artifact_cache_writes =
                self.stats.artifact_cache_writes.saturating_add(written);
            self.stats.artifact_cache_write_transactions = self
                .stats
                .artifact_cache_write_transactions
                .saturating_add(1);
        }

        let completed_work = chunk
            .storages
            .len()
            .saturating_add(chunk.policy_exclusions.len());
        for _ in chunk.progress_already_emitted..completed_work {
            progress.emit();
        }
        for exclusion in chunk.policy_exclusions {
            if self.mode == codestory_workspace::BuildMode::Incremental
                && self
                    .existing_projection_file_ids
                    .contains(&exclusion.file_id)
            {
                // This removal's affected callers are deliberately not folded
                // into the resolution scope. A policy exclusion only ever
                // removes a structural file, and no structural removal is yet
                // known to strand a caller the resolution pass would repair;
                // the plan-driven removal in `run_with_policy_exclusions` is
                // the one that does. The store reports the callers either way,
                // so carrying them is a one-line union once such a case exists.
                self.storage
                    .delete_files_batch(&[exclusion.file_id])
                    .map_err(|error| anyhow!("Storage policy-exclusion cleanup error: {error}"))?;
                self.replaced_projection_ids.insert(exclusion.file_id);
                self.stats.graph_projection_changed = true;
                self.batched_source_identity_only = false;
            }
            self.policy_exclusions.push(exclusion.candidate);
        }
        for local_storage in chunk.storages {
            self.accept_storage(local_storage)?;
        }
        if self.pipeline_telemetry {
            self.stats.full_refresh_chunks_persisted =
                self.stats.full_refresh_chunks_persisted.saturating_add(1);
        }
        Ok(())
    }

    fn accept_storage(&mut self, mut local_storage: IntermediateStorage) -> Result<()> {
        // A verified malformed snapshot replaces the old projection with source
        // identity only. An operational failure cannot authorize that removal.
        let verified_malformed = !local_storage.file_content_hashes.is_empty()
            && !local_storage.errors.is_empty()
            && local_storage
                .errors
                .iter()
                .all(|error| error.coverage_reason == Some(FileCoverageReason::Malformed));
        let mut source_identity_only = false;
        if let Some((file_id, file_complete, file_path)) = local_storage
            .files
            .first()
            .map(|file_info| (file_info.id, file_info.complete, file_info.path.clone()))
            && self.mode == codestory_workspace::BuildMode::Incremental
            && self.existing_projection_file_ids.contains(&file_id)
            && self.replaced_projection_ids.insert(file_id)
        {
            let previous_file = self
                .storage
                .get_files_by_paths(std::slice::from_ref(&file_path))
                .map_err(|error| anyhow!("Storage file lookup error: {error}"))?
                .remove(&file_path);
            let existing_states = self
                .storage
                .get_callable_projection_states_for_file(file_id)
                .map_err(|e| anyhow!("Storage state lookup error: {:?}", e))?;
            let replace_file_owned_projection = verified_malformed
                || !local_storage.structural_text_projections.is_empty()
                || local_storage
                    .files
                    .first()
                    .is_some_and(|file| file.language == "openapi");
            let update_mode = if replace_file_owned_projection {
                ProjectionUpdateMode::FullReplace
            } else {
                classify_projection_update(
                    &existing_states,
                    &local_storage.callable_projection_states,
                )
            };
            source_identity_only = matches!(&update_mode, ProjectionUpdateMode::NoChanges)
                && previous_file.as_ref().is_some_and(|previous| {
                    local_storage.files.first().is_some_and(|current| {
                        previous.complete == current.complete
                            && previous.language == current.language
                            && previous.file_role == current.file_role
                    })
                });
            if source_identity_only {
                // The callable and file-structural fences prove the graph is
                // unchanged. Keep the inherited immutable rows and flush only
                // the new file identity, content hash, and diagnostics.
                local_storage
                    .nodes
                    .retain(|node| node.id == NodeId(file_id));
                local_storage.structural_unit_node_ids.clear();
                local_storage.structural_text_units.clear();
                local_storage.structural_text_projections.clear();
                local_storage.structural_text_cache_writes.clear();
                local_storage.edges.clear();
                local_storage.occurrences.clear();
                local_storage.component_access.clear();
                local_storage.callable_projection_states.clear();
                local_storage.impl_anchor_node_ids.clear();
                self.stats.source_identity_only_files =
                    self.stats.source_identity_only_files.saturating_add(1);
            } else if !file_complete && !verified_malformed {
                // An unreadable, drifting, oversized, or parser-partial source is retry
                // evidence, not proof that its previous symbols disappeared. Preserve the
                // last verified projection and update only the file/error rows below.
                local_storage
                    .nodes
                    .retain(|node| node.id != NodeId(file_id));
                self.stats.graph_projection_changed = true;
            } else {
                let cleanup_started = Instant::now();
                match update_mode {
                    ProjectionUpdateMode::InsertFresh | ProjectionUpdateMode::NoChanges => {}
                    ProjectionUpdateMode::Delta { changed_callers } => {
                        self.storage
                            .delete_projection_for_callers(file_id, &changed_callers)
                            .map_err(|e| anyhow!("Storage delta cleanup error: {:?}", e))?;
                    }
                    ProjectionUpdateMode::RepositionUnowned { changed_callers } => {
                        // Order matters: the unowned cleanup reads ownership off
                        // the stored callable rows, and the caller cleanup
                        // deletes them.
                        self.storage
                            .delete_unowned_projection_for_file(file_id)
                            .map_err(|e| anyhow!("Storage reposition cleanup error: {:?}", e))?;
                        self.storage
                            .delete_projection_for_callers(file_id, &changed_callers)
                            .map_err(|e| anyhow!("Storage delta cleanup error: {:?}", e))?;
                    }
                    ProjectionUpdateMode::FullReplace => {
                        self.storage
                            .delete_file_projection(file_id)
                            .map_err(|e| anyhow!("Storage cleanup error: {:?}", e))?;
                    }
                }
                self.stats.graph_projection_changed = true;
                self.stats.cleanup_ms = self
                    .stats
                    .cleanup_ms
                    .saturating_add(duration_ms_u64(cleanup_started.elapsed()));
            }
        } else if self.mode == codestory_workspace::BuildMode::Incremental
            && !local_storage.files.is_empty()
        {
            self.stats.graph_projection_changed = true;
        }
        let owning_file_ids = self
            .batched_storage
            .files
            .iter()
            .chain(&local_storage.files)
            .map(|file| file.id)
            .collect::<HashSet<_>>();
        for error in local_storage.errors.drain(..) {
            match error.file_id {
                Some(file_id) if owning_file_ids.contains(&file_id.0) => {
                    self.pending_file_errors.push(error);
                }
                Some(file_id) => {
                    self.fallback_file_error_ids.insert(file_id.0);
                    self.fallback_file_errors.push(error);
                }
                None => self.all_errors.push(error),
            }
        }
        let incoming_file_ids = local_storage
            .files
            .iter()
            .map(|file| file.id)
            .collect::<HashSet<_>>();
        if !incoming_file_ids.is_empty() {
            // Refresh plans can repeat one path. Preserve their serial last-write
            // semantics while keeping the store batch contract unambiguous.
            self.batched_storage
                .file_content_hashes
                .retain(|identity| !incoming_file_ids.contains(&identity.file_id));
            self.batched_storage
                .structural_text_units
                .retain(|unit| !incoming_file_ids.contains(&unit.file_id));
            self.batched_storage
                .structural_text_projections
                .retain(|projection| !incoming_file_ids.contains(&projection.file_id));
            self.batched_storage
                .structural_text_cache_writes
                .retain(|write| !incoming_file_ids.contains(&write.file_id));
        }
        self.batched_source_identity_only &= source_identity_only;
        self.batched_storage.merge(local_storage);

        let should_flush = !self.batched_storage.files.is_empty()
            || !self.batched_storage.nodes.is_empty()
            || !self.batched_storage.edges.is_empty()
            || !self.batched_storage.occurrences.is_empty();
        let pipeline_flush = std::env::var("CODESTORY_PIPELINE_FLUSH")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            });
        if should_flush
            && (pipeline_flush
                || self.batched_storage.nodes.len() >= self.batch_config.node_batch_size
                || self.batched_storage.edges.len() >= self.batch_config.edge_batch_size
                || self.batched_storage.occurrences.len()
                    >= self.batch_config.occurrence_batch_size)
        {
            let breakdown = WorkspaceIndexer::flush_projection_batch(
                self.storage,
                &mut self.batched_storage,
                &mut self.pending_file_errors,
                &mut self.had_edges,
                &mut self.stats,
                self.batched_source_identity_only,
            )?;
            self.batched_source_identity_only = true;
            accumulate_flush_breakdown(&mut self.stats, breakdown);
            WorkspaceIndexer::flush_fallback_file_errors(
                self.storage,
                &mut self.fallback_file_error_ids,
                &mut self.fallback_file_errors,
                &mut self.stats,
            )?;
        }

        if self.all_errors.len() >= self.batch_config.error_batch_size {
            let error_flush_started = Instant::now();
            WorkspaceIndexer::flush_errors(
                self.storage,
                &mut self.all_errors,
                self.batch_config.error_batch_size,
            )?;
            self.stats.error_flush_ms = self
                .stats
                .error_flush_ms
                .saturating_add(duration_ms_u64(error_flush_started.elapsed()));
        }
        Ok(())
    }

    fn finish(mut self) -> Result<ProjectionWriterOutput> {
        let breakdown = WorkspaceIndexer::flush_projection_batch(
            self.storage,
            &mut self.batched_storage,
            &mut self.pending_file_errors,
            &mut self.had_edges,
            &mut self.stats,
            self.batched_source_identity_only,
        )?;
        accumulate_flush_breakdown(&mut self.stats, breakdown);
        WorkspaceIndexer::flush_fallback_file_errors(
            self.storage,
            &mut self.fallback_file_error_ids,
            &mut self.fallback_file_errors,
            &mut self.stats,
        )?;
        Ok(ProjectionWriterOutput {
            stats: self.stats,
            all_errors: self.all_errors,
            had_edges: self.had_edges,
            policy_exclusions: self.policy_exclusions,
        })
    }
}

/// Workspace-level indexer that executes refresh plans into a `Store`.
///
/// The indexer does not discover stale files; it consumes a
/// `RefreshExecutionPlan` from `codestory-workspace`. Incremental runs seed
/// enough existing symbol state to preserve stable projections, while full
/// refreshes rebuild from the scheduled files.
pub struct WorkspaceIndexer {
    root: PathBuf,
    compilation_db: Option<compilation_database::CompilationDatabase>,
    compilation_db_warning: Option<String>,
    batch_config: IncrementalIndexingConfig,
    full_refresh_chunk_budget: FullRefreshChunkBudget,
    source_file_byte_cap: u64,
    structural_source_byte_cap: u64,
    source_index_policy: Option<SourceIndexPolicy>,
    artifact_cache_policies: ArtifactCachePolicies,
    #[cfg(test)]
    pipeline_test_hooks: FullRefreshPipelineTestHooks,
    #[cfg(test)]
    before_resolution_test_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl WorkspaceIndexer {
    /// Create an indexer rooted at a workspace directory.
    ///
    /// If a compilation database is present, C/C++ header routing can use it;
    /// load failures are reported later through the event bus and indexing
    /// continues without that metadata.
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
            full_refresh_chunk_budget: FullRefreshChunkBudget::default(),
            source_file_byte_cap: SourceIndexPolicy::default().byte_cap,
            structural_source_byte_cap: SourceIndexPolicy::default().structural_byte_cap,
            source_index_policy: None,
            artifact_cache_policies: ArtifactCachePolicies::default(),
            #[cfg(test)]
            pipeline_test_hooks: FullRefreshPipelineTestHooks::default(),
            #[cfg(test)]
            before_resolution_test_hook: None,
        }
    }

    /// Override incremental flush batch sizes.
    ///
    /// An explicit file batch size also becomes the full-refresh file-count
    /// safety ceiling; normal full refreshes use the adaptive default.
    pub fn with_batch_config(mut self, batch_config: IncrementalIndexingConfig) -> Self {
        self.batch_config = batch_config;
        self.full_refresh_chunk_budget.file_ceiling = batch_config.file_batch_size.max(1);
        self
    }

    #[cfg(test)]
    fn with_full_refresh_chunk_budget(mut self, budget: FullRefreshChunkBudget) -> Self {
        self.full_refresh_chunk_budget = AdaptiveFullRefreshChunkPlanner::new(budget).budget;
        self
    }

    /// Override the parser-backed source file byte cap.
    pub fn with_source_file_byte_cap(mut self, source_file_byte_cap: u64) -> Self {
        self.source_file_byte_cap = source_file_byte_cap.max(1);
        self
    }

    /// Enable verified structural-unit exclusions under one caller-owned policy.
    pub fn with_source_index_policy(mut self, policy: SourceIndexPolicy) -> Self {
        self.source_file_byte_cap = policy.byte_cap;
        // Clamped for the same reason `effective_byte_cap` clamps: an operator
        // lowering the headroom below the structural bound must not leave the
        // collector admitting above it.
        self.structural_source_byte_cap = policy.structural_byte_cap.min(policy.byte_cap);
        self.source_index_policy = Some(policy);
        self
    }

    /// Select cache access independently for parser-backed and structural artifacts.
    pub fn with_artifact_cache_policies(mut self, policies: ArtifactCachePolicies) -> Self {
        self.artifact_cache_policies = policies;
        self
    }

    #[cfg(test)]
    fn with_pipeline_test_hooks(mut self, hooks: FullRefreshPipelineTestHooks) -> Self {
        self.pipeline_test_hooks = hooks;
        self
    }

    #[cfg(test)]
    fn with_before_resolution_test_hook(mut self, hook: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.before_resolution_test_hook = Some(hook);
        self
    }

    /// Return the workspace root path used to normalize refresh-plan inputs.
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// Run an incremental plan built from legacy `RefreshInfo`.
    pub fn run_incremental(
        &self,
        storage: &mut Storage,
        refresh_info: &codestory_workspace::RefreshInfo,
        event_bus: &EventBus,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<IncrementalIndexingStats> {
        let existing_file_ids =
            Self::existing_projection_ids(storage, &self.root, &refresh_info.files_to_index)?;
        let plan = codestory_workspace::RefreshExecutionPlan {
            mode: codestory_workspace::BuildMode::Incremental,
            files_to_index: refresh_info.files_to_index.clone(),
            files_to_remove: refresh_info.files_to_remove.clone(),
            existing_file_ids,
        };
        self.run(storage, &plan, event_bus, cancel_token)
    }

    /// Execute a workspace refresh plan and flush projections into storage.
    ///
    /// The plan supplies the freshness decision. This method parses scheduled
    /// files, routes structural candidates to structural collectors, flushes
    /// graph/search projection batches, and publishes progress. Cancellation is
    /// cooperative and returns stats for completed work.
    pub fn run(
        &self,
        storage: &mut Storage,
        plan: &codestory_workspace::RefreshExecutionPlan,
        event_bus: &EventBus,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<IncrementalIndexingStats> {
        Ok(self
            .run_with_policy_exclusions(storage, plan, event_bus, cancel_token)?
            .stats)
    }

    /// Execute a refresh and return collector-discovered policy exclusions.
    pub fn run_with_policy_exclusions(
        &self,
        storage: &mut Storage,
        plan: &codestory_workspace::RefreshExecutionPlan,
        event_bus: &EventBus,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<WorkspaceIndexingOutcome> {
        let plan = plan.clone();
        event_bus.publish(Event::IndexingStarted {
            file_count: plan.files_to_index.len(),
        });
        if let Some(message) = &self.compilation_db_warning {
            event_bus.publish(Event::ShowWarning {
                message: message.clone(),
            });
        }
        let mut stats = IncrementalIndexingStats {
            parser_artifact_cache: ArtifactCacheFamilyStats::new(
                self.artifact_cache_policies.parser,
            ),
            structural_artifact_cache: ArtifactCacheFamilyStats::new(
                self.artifact_cache_policies.structural,
            ),
            ..IncrementalIndexingStats::default()
        };
        if plan.mode == codestory_workspace::BuildMode::FullRefresh {
            Self::record_full_refresh_chunk_config(&mut stats, self.full_refresh_chunk_budget);
        }
        if Self::is_cancelled(cancel_token) {
            return Ok(WorkspaceIndexingOutcome {
                stats,
                policy_exclusions: Vec::new(),
            });
        }
        let total_files = plan.files_to_index.len();
        let processed_count = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let root = self.root.clone();
        let existing_projection_setup_started = Instant::now();
        let existing_projection_ids = plan.existing_file_ids.clone();
        let existing_projection_file_ids = Self::existing_projection_file_ids(
            storage,
            &root,
            &plan.files_to_index,
            &existing_projection_ids,
        )?;
        stats.setup_existing_projection_ids_ms =
            duration_ms_u64(existing_projection_setup_started.elapsed());
        if Self::is_cancelled(cancel_token) {
            return Ok(WorkspaceIndexingOutcome {
                stats,
                policy_exclusions: Vec::new(),
            });
        }

        let symbol_seed_started = Instant::now();
        let symbol_table = Arc::new(SymbolTable::new());
        Self::seed_symbol_table(
            storage,
            &symbol_table,
            plan.mode,
            &existing_projection_file_ids,
        )?;
        stats.setup_seed_symbol_table_ms = duration_ms_u64(symbol_seed_started.elapsed());

        if Self::is_cancelled(cancel_token) {
            return Ok(WorkspaceIndexingOutcome {
                stats,
                policy_exclusions: Vec::new(),
            });
        }

        // 1. Parallel Indexing (chunked and flushed)
        let full_refresh_defaults =
            IncrementalIndexingConfig::for_mode(codestory_workspace::BuildMode::FullRefresh);
        let batch_config = match plan.mode {
            codestory_workspace::BuildMode::Incremental => self.batch_config,
            codestory_workspace::BuildMode::FullRefresh => IncrementalIndexingConfig {
                file_batch_size: self.batch_config.file_batch_size,
                node_batch_size: self
                    .batch_config
                    .node_batch_size
                    .min(full_refresh_defaults.node_batch_size),
                edge_batch_size: self
                    .batch_config
                    .edge_batch_size
                    .min(full_refresh_defaults.edge_batch_size),
                occurrence_batch_size: self
                    .batch_config
                    .occurrence_batch_size
                    .min(full_refresh_defaults.occurrence_batch_size),
                error_batch_size: self
                    .batch_config
                    .error_batch_size
                    .min(full_refresh_defaults.error_batch_size),
            },
        };
        let writer_output = if plan.mode == codestory_workspace::BuildMode::FullRefresh
            && Self::full_refresh_pipeline_paths_are_unique(&root, &plan.files_to_index)
        {
            let cache_read_plan = self.full_refresh_cache_read_plan(&root, &plan.files_to_index);
            let needs_cache_reader = cache_read_plan.reader_owner.is_some();
            let cache_reader = needs_cache_reader
                .then(|| storage.index_artifact_cache_reader())
                .transpose()?
                .flatten();
            if cache_reader.is_some() || !needs_cache_reader {
                if cache_reader.is_some() {
                    match cache_read_plan.reader_owner {
                        Some(ArtifactCacheFamily::Parser) => {
                            stats.parser_artifact_cache.reader_opens =
                                stats.parser_artifact_cache.reader_opens.saturating_add(1);
                        }
                        Some(ArtifactCacheFamily::Structural) => {
                            stats.structural_artifact_cache.reader_opens = stats
                                .structural_artifact_cache
                                .reader_opens
                                .saturating_add(1);
                        }
                        None => unreachable!("opened cache reader must have one owning family"),
                    }
                }
                self.run_full_refresh_pipeline(
                    storage,
                    cache_reader.as_ref(),
                    &plan.files_to_index,
                    &root,
                    batch_config,
                    &existing_projection_file_ids,
                    &symbol_table,
                    &processed_count,
                    total_files,
                    event_bus,
                    &cancelled,
                    cancel_token,
                    &mut stats,
                )?
            } else {
                self.run_serial_index_chunks(
                    storage,
                    &plan,
                    &root,
                    batch_config,
                    &existing_projection_file_ids,
                    &symbol_table,
                    &processed_count,
                    total_files,
                    event_bus,
                    &cancelled,
                    cancel_token,
                    &mut stats,
                )?
            }
        } else {
            self.run_serial_index_chunks(
                storage,
                &plan,
                &root,
                batch_config,
                &existing_projection_file_ids,
                &symbol_table,
                &processed_count,
                total_files,
                event_bus,
                &cancelled,
                cancel_token,
                &mut stats,
            )?
        };
        accumulate_projection_writer_stats(&mut stats, &writer_output.stats);
        let mut all_errors = writer_output.all_errors;
        let had_edges = writer_output.had_edges;
        let policy_exclusions = writer_output.policy_exclusions;

        if cancelled.load(Ordering::Relaxed) {
            event_bus.publish(Event::IndexingComplete { duration_ms: 0 });
            return Ok(WorkspaceIndexingOutcome {
                stats,
                policy_exclusions,
            });
        }

        // Removal clears the resolution of every surviving edge that pointed
        // into a removed file. Those callers are not in `files_to_index`, so
        // without carrying them forward the resolution scope would skip them
        // and a deleted preferred definition would leave its callers dangling
        // until the next full rebuild.
        let mut removal_affected_caller_file_ids: HashSet<i64> = HashSet::new();
        if plan.mode == codestory_workspace::BuildMode::Incremental
            && !plan.files_to_remove.is_empty()
        {
            stats.graph_projection_changed = true;
            let cleanup_started = Instant::now();
            let removal = storage
                .delete_files_batch(&plan.files_to_remove)
                .map_err(|e| anyhow!("Storage cleanup error: {:?}", e))?;
            removal_affected_caller_file_ids.extend(removal.affected_caller_file_ids);
            stats.cleanup_ms = stats
                .cleanup_ms
                .saturating_add(duration_ms_u64(cleanup_started.elapsed()));
        }

        if Self::is_cancelled(cancel_token) {
            event_bus.publish(Event::IndexingComplete { duration_ms: 0 });
            return Ok(WorkspaceIndexingOutcome {
                stats,
                policy_exclusions,
            });
        }

        // 3.4 Complete the pending same-root TYPE_USAGE channel now that all
        // of the run's declarations are flushed (producer-side, not part of
        // the resolution pipeline; see `finalize_pending_type_usage_edges`).
        finalize_pending_type_usage_edges(storage)?;

        // 3.5 Resolve call/import edges post-pass
        let (resolution_scope_file_ids, expanded_resolution_scope_files) =
            if plan.mode == codestory_workspace::BuildMode::Incremental {
                let mut file_ids = Self::collect_touched_file_ids(&root, &plan.files_to_index);
                // Expansion looks for callers unblocked by *new* definitions, so
                // it runs against the touched set only; the removal's affected
                // callers are unioned in afterwards and widen nothing else.
                let expanded = Self::extend_resolution_scope_for_matching_unresolved_targets(
                    storage,
                    &mut file_ids,
                )?;
                file_ids.extend(removal_affected_caller_file_ids.iter().copied());
                (file_ids, expanded)
            } else {
                (HashSet::new(), 0)
            };
        if stats.graph_projection_changed
            && (had_edges
                || expanded_resolution_scope_files > 0
                || !removal_affected_caller_file_ids.is_empty())
        {
            let resolver = resolution::ResolutionPass::new();
            let resolution_scope = if plan.mode == codestory_workspace::BuildMode::Incremental {
                (!resolution_scope_file_ids.is_empty()).then_some(&resolution_scope_file_ids)
            } else {
                None
            };
            let (unresolved_calls_start, unresolved_imports_start) =
                resolver.unresolved_counts_with_scope(storage, resolution_scope)?;
            let unresolved_overrides_start = resolver.unresolved_edge_count_with_scope(
                storage,
                EdgeKind::OVERRIDE,
                resolution_scope,
            )?;
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
            #[cfg(test)]
            if let Some(hook) = &self.before_resolution_test_hook {
                hook();
            }
            let resolution_started = Instant::now();
            let resolution_stats = match resolver.run_with_scope_with_cancel(
                storage,
                resolution_scope,
                cancel_token,
            ) {
                Ok(resolution_stats) => resolution_stats,
                Err(error) if resolution::is_resolution_cancelled(&error) => {
                    event_bus.publish(Event::IndexingComplete { duration_ms: 0 });
                    return Ok(WorkspaceIndexingOutcome {
                        stats,
                        policy_exclusions,
                    });
                }
                Err(error) => return Err(anyhow!("Resolution error: {:?}", error)),
            };
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
            stats.resolution_override_count_ms =
                resolution_stats.telemetry.unresolved_override_count_ms;
            stats.resolution_call_candidate_index_ms =
                resolution_stats.telemetry.call_candidate_index_ms;
            stats.resolution_import_candidate_index_ms =
                resolution_stats.telemetry.import_candidate_index_ms;
            stats.resolution_call_semantic_index_ms =
                resolution_stats.telemetry.call_semantic_index_ms;
            stats.resolution_import_semantic_index_ms =
                resolution_stats.telemetry.import_semantic_index_ms;
            stats.resolution_support_snapshot_load_ms =
                resolution_stats.telemetry.support_snapshot_load_ms;
            stats.resolution_support_snapshot_store_ms =
                resolution_stats.telemetry.support_snapshot_store_ms;
            stats.resolution_support_snapshot_hit = resolution_stats.telemetry.support_snapshot_hit;
            stats.resolution_support_snapshot_limit_bytes =
                resolution_stats.telemetry.support_snapshot_limit_bytes;
            stats.resolution_support_snapshot_stored =
                resolution_stats.telemetry.support_snapshot_stored;
            stats.resolution_support_snapshot_skipped_oversize =
                resolution_stats.telemetry.support_snapshot_skipped_oversize;
            stats.resolution_call_semantic_candidates_ms =
                resolution_stats.telemetry.call_semantic_candidates_ms;
            stats.resolution_import_semantic_candidates_ms =
                resolution_stats.telemetry.import_semantic_candidates_ms;
            stats.resolution_call_semantic_requests =
                resolution_stats.telemetry.call_semantic_requests;
            stats.resolution_call_semantic_unique_requests =
                resolution_stats.telemetry.call_semantic_unique_requests;
            stats.resolution_call_semantic_skipped_requests =
                resolution_stats.telemetry.call_semantic_skipped_requests;
            stats.resolution_import_semantic_requests =
                resolution_stats.telemetry.import_semantic_requests;
            stats.resolution_import_semantic_unique_requests =
                resolution_stats.telemetry.import_semantic_unique_requests;
            stats.resolution_import_semantic_skipped_requests =
                resolution_stats.telemetry.import_semantic_skipped_requests;
            stats.resolution_call_compute_ms = resolution_stats.telemetry.call_compute_ms;
            stats.resolution_import_compute_ms = resolution_stats.telemetry.import_compute_ms;
            stats.resolution_call_apply_ms = resolution_stats.telemetry.call_apply_ms;
            stats.resolution_import_apply_ms = resolution_stats.telemetry.import_apply_ms;
            stats.resolution_override_resolution_ms =
                resolution_stats.telemetry.override_resolution_ms;
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
            stats.resolved_calls_same_module = resolution_stats.strategy_counters.call_same_module;
            stats.resolved_calls_global_unique =
                resolution_stats.strategy_counters.call_global_unique;
            stats.resolved_calls_semantic =
                resolution_stats.strategy_counters.call_semantic_fallback;
            stats.resolved_imports_same_file = resolution_stats.strategy_counters.import_same_file;
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

        event_bus.publish(Event::IndexingComplete { duration_ms: 0 });
        Ok(WorkspaceIndexingOutcome {
            stats,
            policy_exclusions,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_index_chunk(
        &self,
        cache_access: &mut ArtifactCacheAccess<'_>,
        _chunk_index: usize,
        file_chunk: &[PathBuf],
        root: &Path,
        mode: codestory_workspace::BuildMode,
        existing_projection_file_ids: &HashSet<i64>,
        symbol_table: &Arc<SymbolTable>,
        cancelled: &AtomicBool,
        cancel_token: Option<&CancellationToken>,
        serial_progress: Option<IndexProgress<'_>>,
        stats: &mut IncrementalIndexingStats,
    ) -> PreparedIndexChunk {
        #[cfg(test)]
        if let Some(hook) = &self.pipeline_test_hooks.before_prepare_chunk {
            hook(_chunk_index);
        }
        let lookup_started = Instant::now();
        let mut storages = Vec::with_capacity(file_chunk.len());
        let mut parse_jobs = Vec::new();
        for path in file_chunk {
            if let Some(token) = cancel_token
                && token.is_cancelled()
            {
                cancelled.store(true, Ordering::Relaxed);
                break;
            }

            let normalized_path = Self::normalize_index_path(root, path);
            let file_id = Self::canonical_file_node_id_for_path(&normalized_path);
            let existing_projection_id = (mode == codestory_workspace::BuildMode::Incremental)
                .then_some(file_id)
                .filter(|file_id| existing_projection_file_ids.contains(file_id));
            match self.prepare_index_work(
                cache_access,
                path,
                root,
                existing_projection_id,
                symbol_table,
                stats,
            ) {
                Ok(PreparedIndexWork::Immediate(local_storage)) => {
                    if let Some(progress) = serial_progress {
                        progress.emit();
                    }
                    storages.push(local_storage);
                }
                Ok(PreparedIndexWork::Parse(prepared_input)) => {
                    parse_jobs.push(PreparedIndexJob::Parse(Box::new(prepared_input)))
                }
                Ok(PreparedIndexWork::Structural(prepared_input)) => {
                    parse_jobs.push(PreparedIndexJob::Structural(prepared_input))
                }
                Err(err_storage) => {
                    if let Some(progress) = serial_progress {
                        progress.emit();
                    }
                    storages.push(err_storage);
                }
            }
        }
        let progress_already_emitted = serial_progress.map_or(0, |_| storages.len());
        let source_prepare_ms = duration_ms_u64(lookup_started.elapsed());
        stats.source_prepare_ms = stats.source_prepare_ms.saturating_add(source_prepare_ms);
        stats.artifact_cache_lookup_ms = stats
            .artifact_cache_lookup_ms
            .saturating_add(source_prepare_ms);

        let parse_started = Instant::now();
        let parse_results: Vec<PreparedIndexJobResult> = parse_jobs
            .par_iter()
            .map(|prepared_input| {
                #[cfg(test)]
                if let Some(hook) = &self.pipeline_test_hooks.before_parse_job {
                    hook(_chunk_index);
                }
                if let Some(token) = cancel_token
                    && token.is_cancelled()
                {
                    cancelled.store(true, Ordering::Relaxed);
                    return PreparedIndexJobResult {
                        local_storage: IntermediateStorage::default(),
                        cache_write: None,
                        policy_exclusion: None,
                    };
                }
                match prepared_input {
                    PreparedIndexJob::Parse(prepared_input) => {
                        self.execute_prepared_index(prepared_input, symbol_table)
                    }
                    PreparedIndexJob::Structural(prepared_input) => {
                        self.execute_prepared_structural_index(prepared_input)
                    }
                }
            })
            .collect();
        stats.parse_index_ms = stats
            .parse_index_ms
            .saturating_add(duration_ms_u64(parse_started.elapsed()));
        // Parsed projections and serialized cache artifacts own everything the
        // writer needs. Release source strings before a capacity wait.
        drop(parse_jobs);

        let mut cache_writes = Vec::with_capacity(parse_results.len());
        let mut policy_exclusions = Vec::new();
        for parsed in parse_results {
            if let Some(cache_write) = parsed.cache_write {
                cache_writes.push(cache_write);
            }
            if let Some(exclusion) = parsed.policy_exclusion {
                policy_exclusions.push(exclusion);
            } else {
                storages.push(parsed.local_storage);
            }
        }
        PreparedIndexChunk {
            cache_writes,
            storages,
            policy_exclusions,
            progress_already_emitted,
            #[cfg(test)]
            before_persist: self
                .pipeline_test_hooks
                .before_writer_chunk
                .as_ref()
                .map(|hook| (_chunk_index, hook.clone())),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_serial_index_chunks(
        &self,
        storage: &mut Storage,
        plan: &codestory_workspace::RefreshExecutionPlan,
        root: &Path,
        batch_config: IncrementalIndexingConfig,
        existing_projection_file_ids: &HashSet<i64>,
        symbol_table: &Arc<SymbolTable>,
        processed_count: &AtomicUsize,
        total_files: usize,
        event_bus: &EventBus,
        cancelled: &AtomicBool,
        cancel_token: Option<&CancellationToken>,
        stats: &mut IncrementalIndexingStats,
    ) -> Result<ProjectionWriterOutput> {
        let progress = IndexProgress {
            processed_count,
            total_files,
            event_bus,
        };
        let mut writer = ProjectionWriter::new(
            storage,
            plan.mode,
            batch_config,
            existing_projection_file_ids,
            false,
        );
        if plan.mode == codestory_workspace::BuildMode::FullRefresh {
            let mut planner = AdaptiveFullRefreshChunkPlanner::new(self.full_refresh_chunk_budget);
            #[cfg(test)]
            planner.set_before_plan_file_hook(self.pipeline_test_hooks.before_plan_file.clone());
            let mut start = 0usize;
            let mut chunk_index = 0usize;
            let mut planning_duration = Duration::ZERO;
            loop {
                let planning_started = Instant::now();
                let next_chunk =
                    planner.next_chunk(&plan.files_to_index, root, start, cancel_token);
                planning_duration = planning_duration.saturating_add(planning_started.elapsed());
                if Self::is_cancelled(cancel_token) {
                    cancelled.store(true, Ordering::Relaxed);
                    break;
                }
                let Some(chunk_plan) = next_chunk else {
                    break;
                };
                let chunk = {
                    let mut cache_access = ArtifactCacheAccess::storage(
                        writer.storage_mut(),
                        self.artifact_cache_policies,
                    );
                    self.prepare_index_chunk(
                        &mut cache_access,
                        chunk_index,
                        &plan.files_to_index[chunk_plan.start..chunk_plan.end],
                        root,
                        plan.mode,
                        existing_projection_file_ids,
                        symbol_table,
                        cancelled,
                        cancel_token,
                        Some(progress),
                        stats,
                    )
                };
                let node_count = chunk.node_count();
                Self::record_full_refresh_chunk(stats, planner.budget, chunk_plan, node_count);
                planner.observe(chunk_plan.source_bytes, node_count);
                writer.accept_chunk(chunk, progress)?;
                start = chunk_plan.end;
                chunk_index = chunk_index.saturating_add(1);
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }
            }
            stats.full_refresh_chunk_planning_ms = stats
                .full_refresh_chunk_planning_ms
                .saturating_add(duration_ms_u64(planning_duration));
        } else {
            for (chunk_index, file_chunk) in plan
                .files_to_index
                .chunks(batch_config.file_batch_size.max(1))
                .enumerate()
            {
                let chunk = {
                    let mut cache_access = ArtifactCacheAccess::storage(
                        writer.storage_mut(),
                        self.artifact_cache_policies,
                    );
                    self.prepare_index_chunk(
                        &mut cache_access,
                        chunk_index,
                        file_chunk,
                        root,
                        plan.mode,
                        existing_projection_file_ids,
                        symbol_table,
                        cancelled,
                        cancel_token,
                        Some(progress),
                        stats,
                    )
                };
                writer.accept_chunk(chunk, progress)?;
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }
            }
        }
        writer.finish()
    }

    #[allow(clippy::too_many_arguments)]
    fn run_full_refresh_pipeline(
        &self,
        storage: &mut Storage,
        cache_reader: Option<&IndexArtifactCacheReader>,
        files_to_index: &[PathBuf],
        root: &Path,
        batch_config: IncrementalIndexingConfig,
        existing_projection_file_ids: &HashSet<i64>,
        symbol_table: &Arc<SymbolTable>,
        processed_count: &AtomicUsize,
        total_files: usize,
        event_bus: &EventBus,
        cancelled: &AtomicBool,
        cancel_token: Option<&CancellationToken>,
        stats: &mut IncrementalIndexingStats,
    ) -> Result<ProjectionWriterOutput> {
        const QUEUE_CAPACITY: usize = 1;
        const SEND_RETRY: Duration = Duration::from_millis(25);

        let (sender, receiver) = bounded(QUEUE_CAPACITY);
        stats.full_refresh_queue_capacity = QUEUE_CAPACITY;

        std::thread::scope(|scope| {
            let writer_handle = scope.spawn(|| {
                Self::consume_full_refresh_chunks(
                    storage,
                    receiver,
                    batch_config,
                    existing_projection_file_ids,
                    processed_count,
                    total_files,
                    event_bus,
                )
            });

            let producer_result = (|| -> Result<()> {
                let mut planner =
                    AdaptiveFullRefreshChunkPlanner::new(self.full_refresh_chunk_budget);
                #[cfg(test)]
                planner
                    .set_before_plan_file_hook(self.pipeline_test_hooks.before_plan_file.clone());
                let mut start = 0usize;
                let mut chunk_index = 0usize;
                let mut planning_duration = Duration::ZERO;
                loop {
                    let planning_started = Instant::now();
                    let next_chunk = planner.next_chunk(files_to_index, root, start, cancel_token);
                    planning_duration =
                        planning_duration.saturating_add(planning_started.elapsed());
                    if Self::is_cancelled(cancel_token) {
                        cancelled.store(true, Ordering::Relaxed);
                        break;
                    }
                    let Some(chunk_plan) = next_chunk else {
                        break;
                    };
                    let chunk = {
                        let mut cache_access =
                            ArtifactCacheAccess::reader(cache_reader, self.artifact_cache_policies);
                        self.prepare_index_chunk(
                            &mut cache_access,
                            chunk_index,
                            &files_to_index[chunk_plan.start..chunk_plan.end],
                            root,
                            codestory_workspace::BuildMode::FullRefresh,
                            existing_projection_file_ids,
                            symbol_table,
                            cancelled,
                            cancel_token,
                            None,
                            stats,
                        )
                    };
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

                    let node_count = chunk.node_count();
                    let blocked_started = Instant::now();
                    let mut pending_chunk = chunk;
                    let mut accepted = false;
                    loop {
                        match sender.send_timeout(pending_chunk, SEND_RETRY) {
                            Ok(()) => {
                                accepted = true;
                                stats.full_refresh_chunks_produced =
                                    stats.full_refresh_chunks_produced.saturating_add(1);
                                // A successful bounded send has one linearization
                                // point at which the capacity-1 queue is occupied,
                                // even if the receiver immediately dequeues it.
                                stats.full_refresh_queue_high_water = 1;
                                #[cfg(test)]
                                if let Some(hook) = &self.pipeline_test_hooks.after_send_chunk {
                                    hook(chunk_index);
                                }
                                break;
                            }
                            Err(SendTimeoutError::Timeout(chunk)) => {
                                #[cfg(test)]
                                if let Some(hook) = &self.pipeline_test_hooks.on_send_timeout {
                                    hook(chunk_index);
                                }
                                if Self::is_cancelled(cancel_token) {
                                    cancelled.store(true, Ordering::Relaxed);
                                    break;
                                }
                                pending_chunk = chunk;
                            }
                            Err(SendTimeoutError::Disconnected(_)) => {
                                return Err(anyhow!("Full-refresh projection writer disconnected"));
                            }
                        }
                    }
                    stats.full_refresh_producer_blocked_ms = stats
                        .full_refresh_producer_blocked_ms
                        .saturating_add(duration_ms_u64(blocked_started.elapsed()));
                    if accepted {
                        Self::record_full_refresh_chunk(
                            stats,
                            planner.budget,
                            chunk_plan,
                            node_count,
                        );
                        planner.observe(chunk_plan.source_bytes, node_count);
                        start = chunk_plan.end;
                        chunk_index = chunk_index.saturating_add(1);
                    }
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                }
                stats.full_refresh_chunk_planning_ms = stats
                    .full_refresh_chunk_planning_ms
                    .saturating_add(duration_ms_u64(planning_duration));
                Ok(())
            })();
            drop(sender);

            let writer_result = writer_handle
                .join()
                .map_err(|_| anyhow!("Full-refresh projection writer panicked"))?;
            match (producer_result, writer_result) {
                (_, Err(writer_error)) => Err(writer_error),
                (Err(producer_error), _) => Err(producer_error),
                (Ok(()), Ok(output)) => Ok(output),
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn consume_full_refresh_chunks(
        storage: &mut Storage,
        receiver: Receiver<PreparedIndexChunk>,
        batch_config: IncrementalIndexingConfig,
        existing_projection_file_ids: &HashSet<i64>,
        processed_count: &AtomicUsize,
        total_files: usize,
        event_bus: &EventBus,
    ) -> Result<ProjectionWriterOutput> {
        let mut writer = ProjectionWriter::new(
            storage,
            codestory_workspace::BuildMode::FullRefresh,
            batch_config,
            existing_projection_file_ids,
            true,
        );
        let progress = IndexProgress {
            processed_count,
            total_files,
            event_bus,
        };
        loop {
            let idle_started = Instant::now();
            match receiver.recv() {
                Ok(chunk) => {
                    writer.stats.full_refresh_writer_idle_ms = writer
                        .stats
                        .full_refresh_writer_idle_ms
                        .saturating_add(duration_ms_u64(idle_started.elapsed()));
                    writer.accept_chunk(chunk, progress)?;
                }
                Err(_) => {
                    writer.stats.full_refresh_writer_idle_ms = writer
                        .stats
                        .full_refresh_writer_idle_ms
                        .saturating_add(duration_ms_u64(idle_started.elapsed()));
                    break;
                }
            }
        }
        writer.finish()
    }

    fn record_full_refresh_chunk(
        stats: &mut IncrementalIndexingStats,
        budget: FullRefreshChunkBudget,
        plan: FullRefreshChunkPlan,
        node_count: usize,
    ) {
        Self::record_full_refresh_chunk_config(stats, budget);
        stats.full_refresh_chunk_max_files = stats
            .full_refresh_chunk_max_files
            .max(plan.end.saturating_sub(plan.start));
        stats.full_refresh_chunk_max_planned_bytes = stats
            .full_refresh_chunk_max_planned_bytes
            .max(plan.source_bytes);
        stats.full_refresh_chunk_max_nodes = stats.full_refresh_chunk_max_nodes.max(node_count);
        if plan.source_bytes > budget.source_bytes
            || plan.projected_nodes > budget.projected_nodes
            || node_count > budget.projected_nodes
        {
            stats.full_refresh_chunk_budget_overruns =
                stats.full_refresh_chunk_budget_overruns.saturating_add(1);
        }
    }

    fn record_full_refresh_chunk_config(
        stats: &mut IncrementalIndexingStats,
        budget: FullRefreshChunkBudget,
    ) {
        stats.full_refresh_chunk_target_bytes = budget.source_bytes;
        stats.full_refresh_chunk_target_nodes = budget.projected_nodes;
        stats.full_refresh_chunk_file_ceiling = budget.file_ceiling;
    }

    fn is_cancelled(cancel_token: Option<&CancellationToken>) -> bool {
        cancel_token
            .map(CancellationToken::is_cancelled)
            .unwrap_or(false)
    }

    fn full_refresh_pipeline_paths_are_unique(root: &Path, files: &[PathBuf]) -> bool {
        let mut identities = HashSet::with_capacity(files.len());
        files.iter().all(|path| {
            // Refresh planning already resolves native filesystem identity. Keep
            // this defensive check lexical so enabling the pipeline does not add
            // one filesystem canonicalization syscall per corpus file.
            identities.insert(Self::normalize_index_path(root, path))
        })
    }

    fn full_refresh_cache_read_plan(
        &self,
        root: &Path,
        files: &[PathBuf],
    ) -> FullRefreshCacheReadPlan {
        let mut plan = FullRefreshCacheReadPlan::default();
        for path in files {
            let full_path = Self::normalize_index_path(root, path);
            if workspace_structural_source_exclusion(root, &full_path).is_some() {
                continue;
            }
            let compilation_info = self
                .compilation_db
                .as_ref()
                .and_then(|database| database.get_parsed_info(&full_path));
            if get_language_config_for_path(&full_path, compilation_info.as_ref()).is_some() {
                plan.parser = true;
                if plan.reader_owner.is_none()
                    && self.artifact_cache_policies.parser.reads_storage()
                {
                    plan.reader_owner = Some(ArtifactCacheFamily::Parser);
                }
            } else if structural::is_structural_candidate_path(&full_path) {
                plan.structural = true;
                if plan.reader_owner.is_none()
                    && self.artifact_cache_policies.structural.reads_storage()
                {
                    plan.reader_owner = Some(ArtifactCacheFamily::Structural);
                }
            }
            if plan.parser && plan.structural && plan.reader_owner.is_some() {
                break;
            }
        }
        plan
    }

    fn seed_symbol_table(
        storage: &Storage,
        symbol_table: &SymbolTable,
        mode: codestory_workspace::BuildMode,
        existing_projection_file_ids: &HashSet<i64>,
    ) -> Result<()> {
        if mode == codestory_workspace::BuildMode::FullRefresh {
            return Ok(());
        }
        let file_ids = existing_projection_file_ids
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let node_kinds = storage
            .get_node_kinds_for_files(&file_ids)
            .map_err(|e| anyhow!("Storage symbol seed error: {:?}", e))?;
        for (node_id, kind) in node_kinds {
            symbol_table.insert(node_id.0, kind);
        }
        Ok(())
    }

    fn existing_projection_file_ids(
        storage: &Storage,
        root: &Path,
        files_to_index: &[PathBuf],
        existing_projection_ids: &HashMap<PathBuf, i64>,
    ) -> Result<HashSet<i64>> {
        let mut candidates = existing_projection_ids
            .values()
            .copied()
            .collect::<HashSet<_>>();
        for path in files_to_index {
            let full_path = Self::normalize_index_path(root, path);
            candidates.insert(Self::canonical_file_node_id_for_path(&full_path));
            if let Ok(canonical) = full_path.canonicalize() {
                candidates.insert(Self::canonical_file_node_id_for_path(&canonical));
            }
        }
        if candidates.is_empty() {
            return Ok(HashSet::new());
        }

        let candidate_ids = candidates.iter().copied().collect::<Vec<_>>();
        let node_kinds = storage
            .get_node_kinds_for_files(&candidate_ids)
            .map_err(|e| anyhow!("Storage file identity lookup error: {:?}", e))?;
        Ok(node_kinds
            .into_iter()
            .filter_map(|(node_id, kind)| {
                (kind == NodeKind::FILE && candidates.contains(&node_id.0)).then_some(node_id.0)
            })
            .collect())
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

    fn extend_resolution_scope_for_matching_unresolved_targets(
        storage: &Storage,
        scope_file_ids: &mut HashSet<i64>,
    ) -> Result<usize> {
        // New definitions can unblock old unresolved callers; keep the scope bounded
        // to callers whose placeholder target names match names in touched files.
        if scope_file_ids.is_empty() {
            return Ok(0);
        }

        let conn = storage.get_connection();
        conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS incremental_resolution_touched_file_ids (
                file_id INTEGER PRIMARY KEY
             );
             DELETE FROM incremental_resolution_touched_file_ids;
             CREATE TEMP TABLE IF NOT EXISTS incremental_resolution_target_names (
                name TEXT PRIMARY KEY
             );
             DELETE FROM incremental_resolution_target_names;",
        )?;

        {
            let mut insert_touched = conn.prepare(
                "INSERT OR IGNORE INTO incremental_resolution_touched_file_ids (file_id)
                 VALUES (?1)",
            )?;
            for file_id in scope_file_ids.iter() {
                insert_touched.execute(rusqlite::params![file_id])?;
            }
        }

        let definition_kind_values = incremental_resolution_target_node_kinds()
            .iter()
            .map(|kind| (*kind as i32).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let definition_name_query = format!(
            "INSERT OR IGNORE INTO incremental_resolution_target_names (name)
             SELECT DISTINCT name FROM (
                 SELECT serialized_name AS name
                 FROM node
                 WHERE file_node_id IN (
                     SELECT file_id FROM incremental_resolution_touched_file_ids
                 )
                   AND kind IN ({definition_kind_values})
                 UNION
                 SELECT qualified_name AS name
                 FROM node
                 WHERE file_node_id IN (
                     SELECT file_id FROM incremental_resolution_touched_file_ids
                 )
                   AND kind IN ({definition_kind_values})
             )
             WHERE name IS NOT NULL AND name <> ''"
        );
        conn.execute(&definition_name_query, [])?;
        let target_name_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM incremental_resolution_target_names",
            [],
            |row| row.get(0),
        )?;
        if target_name_count == 0 {
            return Ok(0);
        }

        let mut stmt = conn.prepare(
            "SELECT DISTINCT COALESCE(caller.file_node_id, e.file_node_id) AS file_id
             FROM edge e
             JOIN node caller ON caller.id = e.source_node_id
             JOIN node target ON target.id = e.target_node_id
             JOIN incremental_resolution_target_names target_name
               ON target_name.name = target.serialized_name
               OR target_name.name = target.qualified_name
             WHERE e.resolved_target_node_id IS NULL
               AND e.kind IN (?1, ?2, ?3)
               AND (e.kind != ?1 OR (e.confidence IS NULL AND e.certainty IS NULL))
               AND COALESCE(caller.file_node_id, e.file_node_id) IS NOT NULL
               AND (target.canonical_id IS NULL OR (
                 target.canonical_id NOT LIKE 'tauri:command:%'
                 AND target.canonical_id NOT LIKE 'openapi:endpoint:%'
                 AND target.canonical_id NOT LIKE 'route_endpoint:%'
                 AND target.canonical_id NOT LIKE 'payload:collection:%'
               ))",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![
                EdgeKind::CALL as i32,
                EdgeKind::IMPORT as i32,
                EdgeKind::OVERRIDE as i32
            ],
            |row| row.get::<_, i64>(0),
        )?;

        let mut added = 0;
        for row in rows {
            if scope_file_ids.insert(row?) {
                added += 1;
            }
        }
        Ok(added)
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
    ) -> Result<HashMap<PathBuf, i64>> {
        let normalized_paths = files_to_index
            .iter()
            .map(|path| Self::normalize_index_path(root, path))
            .collect::<Vec<_>>();
        let files = storage
            .get_files_by_paths(&normalized_paths)
            .map_err(|e| anyhow!("Storage path lookup error: {:?}", e))?;
        Ok(files
            .into_iter()
            .map(|(path, file_info)| (path, file_info.id))
            .collect())
    }

    fn canonical_file_node_id_for_path(path: &Path) -> i64 {
        let file_name = Self::file_identity_path(path);
        let canonical_id = format!("{file_name}:{file_name}:1");
        generate_id(&canonical_id)
    }

    fn file_identity_path(path: &Path) -> String {
        #[cfg(windows)]
        {
            let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            path.to_string_lossy().replace('\\', "/").to_lowercase()
        }
        #[cfg(not(windows))]
        {
            path.to_string_lossy().to_string()
        }
    }

    fn flush_errors(
        storage: &mut Storage,
        errors: &mut Vec<codestory_contracts::graph::ErrorInfo>,
        error_batch_size: usize,
    ) -> Result<()> {
        if errors.is_empty() {
            return Ok(());
        }

        let take_count = errors.len().min(error_batch_size.max(1));
        let drain = errors.drain(..take_count).collect::<Vec<_>>();
        storage
            .insert_errors_batch(&drain)
            .map_err(|e| anyhow!("Storage error: {:?}", e))?;

        Ok(())
    }

    fn flush_fallback_file_errors(
        storage: &mut Storage,
        file_ids: &mut HashSet<i64>,
        errors: &mut Vec<codestory_contracts::graph::ErrorInfo>,
        stats: &mut IncrementalIndexingStats,
    ) -> Result<()> {
        if file_ids.is_empty() {
            debug_assert!(errors.is_empty());
            return Ok(());
        }

        let mut file_ids_batch = file_ids.iter().copied().collect::<Vec<_>>();
        file_ids_batch.sort_unstable();
        let started = Instant::now();
        storage
            .replace_errors_for_files_batch(&file_ids_batch, errors)
            .map_err(|e| anyhow!("Storage file error fallback replacement error: {:?}", e))?;
        stats.error_flush_ms = stats
            .error_flush_ms
            .saturating_add(duration_ms_u64(started.elapsed()));
        file_ids.clear();
        errors.clear();
        Ok(())
    }

    fn flush_projection_batch(
        storage: &mut Storage,
        batched_storage: &mut IntermediateStorage,
        file_errors: &mut Vec<codestory_contracts::graph::ErrorInfo>,
        had_edges: &mut bool,
        stats: &mut IncrementalIndexingStats,
        preserve_graph_derived_state: bool,
    ) -> Result<codestory_store::ProjectionFlushBreakdown> {
        reconcile_rust_impl_anchors(storage, batched_storage)?;
        let has_rows = projection_batch_has_rows(batched_storage);
        let flush_started = has_rows.then(Instant::now);
        let structural_cache_writes = batched_storage
            .structural_text_cache_writes
            .iter()
            .map(|write| codestory_store::StructuralTextArtifactCacheWrite {
                path: &write.path,
                file_id: write.file_id,
                cache_key: &write.cache_key,
                artifact_blob: &write.artifact_blob,
            })
            .collect::<Vec<_>>();
        let projection_batch = codestory_store::ProjectionBatch {
            files: &batched_storage.files,
            file_content_hashes: &batched_storage.file_content_hashes,
            nodes: &batched_storage.nodes,
            structural_text_units: &batched_storage.structural_text_units,
            structural_text_projections: &batched_storage.structural_text_projections,
            structural_text_cache_writes: &structural_cache_writes,
            edges: &batched_storage.edges,
            occurrences: &batched_storage.occurrences,
            component_access: &batched_storage.component_access,
            callable_projection_states: &batched_storage.callable_projection_states,
            file_errors,
        };
        let breakdown = if preserve_graph_derived_state {
            storage
                .projections()
                .flush_source_identity_batch(projection_batch)
        } else {
            storage
                .projections()
                .flush_projection_batch(projection_batch)
        }
        .map_err(|e| anyhow!("Storage error: {:?}", e))?;
        if let Some(flush_started) = flush_started {
            let batch_wall_ms = duration_ms_u64(flush_started.elapsed());
            debug_assert!(batch_wall_ms >= projection_flush_breakdown_ms(&breakdown));
            stats.projection_batch_wall_ms =
                stats.projection_batch_wall_ms.saturating_add(batch_wall_ms);
            stats.projection_batch_transactions =
                stats.projection_batch_transactions.saturating_add(1);
        }
        if !batched_storage.edges.is_empty() {
            *had_edges = true;
        }

        batched_storage.clear();
        file_errors.clear();
        Ok(breakdown)
    }

    #[allow(clippy::result_large_err)]
    fn prepare_index_work(
        &self,
        cache_access: &mut ArtifactCacheAccess<'_>,
        path: &PathBuf,
        root: &Path,
        existing_projection_id: Option<i64>,
        symbol_table: &Arc<SymbolTable>,
        stats: &mut IncrementalIndexingStats,
    ) -> std::result::Result<PreparedIndexWork, IntermediateStorage> {
        let full_path = Self::normalize_index_path(root, path);
        if workspace_structural_source_exclusion(root, &full_path).is_some() {
            return Ok(PreparedIndexWork::Immediate(IntermediateStorage::default()));
        }
        let compilation_info = self
            .compilation_db
            .as_ref()
            .and_then(|db| db.get_parsed_info(&full_path));
        let language_config = get_language_config_for_path(&full_path, compilation_info.as_ref());
        let source_language = language_config
            .as_ref()
            .map(|config| config.language_name)
            .or_else(|| template_pipeline::template_surface_language(&full_path))
            .or_else(|| openapi_path_language_hint(&full_path).then_some("openapi"))
            .or_else(|| {
                structural::is_structural_candidate_path(&full_path)
                    .then(|| structural::structural_language_name(&full_path))
            })
            .or_else(|| {
                is_text_only_candidate_path(&full_path).then(|| text_only_language_name(&full_path))
            })
            .or_else(|| is_openapi_candidate_path(&full_path).then_some("openapi"))
            .or_else(|| companion_inventory_language(&full_path));
        let Some(source_language) = source_language else {
            return Ok(PreparedIndexWork::Immediate(IntermediateStorage::default()));
        };

        let file_size = match std::fs::metadata(&full_path) {
            Ok(metadata) => metadata.len(),
            Err(e) => {
                let local_storage = incomplete_file_storage(
                    &full_path,
                    None,
                    source_language,
                    codestory_contracts::graph::ErrorInfo {
                        message: format!("Failed to inspect {:?}: {}", path, e),
                        file_id: None,
                        line: None,
                        column: None,
                        is_fatal: true,
                        index_step: codestory_contracts::graph::IndexStep::Collection,
                        coverage_reason: Some(FileCoverageReason::Unreadable),
                    },
                );
                return Err(local_storage);
            }
        };
        if file_size > self.source_file_byte_cap {
            let local_storage = incomplete_file_storage(
                &full_path,
                None,
                source_language,
                codestory_contracts::graph::ErrorInfo {
                    message: format!(
                        "Skipped oversized source file {:?}: {} bytes exceeds {} byte cap",
                        path, file_size, self.source_file_byte_cap
                    ),
                    file_id: None,
                    line: None,
                    column: None,
                    is_fatal: false,
                    index_step: codestory_contracts::graph::IndexStep::Indexing,
                    coverage_reason: Some(FileCoverageReason::Oversized),
                },
            );
            return Err(local_storage);
        }

        let Some(mut language_config) = language_config else {
            match self.prepare_openapi_schema_work(&full_path) {
                Ok(Some(local_storage)) => return Ok(PreparedIndexWork::Immediate(local_storage)),
                Ok(None) => {}
                Err(err_storage) => return Err(err_storage),
            }
            if let Some(template_kind) = template_pipeline::template_kind_for_path(&full_path) {
                return match prepare_template_index_work(&full_path, template_kind) {
                    Ok(local_storage) => Ok(PreparedIndexWork::Immediate(local_storage)),
                    Err(error) => {
                        let local_storage = incomplete_file_storage(
                            &full_path,
                            None,
                            template_pipeline::template_surface_language(&full_path)
                                .unwrap_or("template"),
                            codestory_contracts::graph::ErrorInfo {
                                message: format!(
                                    "Failed to index template file {:?}: {}",
                                    path, error
                                ),
                                file_id: None,
                                line: None,
                                column: None,
                                is_fatal: false,
                                index_step: codestory_contracts::graph::IndexStep::Indexing,
                                coverage_reason: Some(FileCoverageReason::CollectorFailure),
                            },
                        );
                        Err(local_storage)
                    }
                };
            }
            if structural::is_structural_candidate_path(&full_path) {
                return self.prepare_structural_index_work(
                    cache_access,
                    path,
                    root,
                    existing_projection_id,
                    stats,
                );
            }
            if is_text_only_candidate_path(&full_path) {
                return match index_text_only_file(&full_path) {
                    Ok(local_storage) => Ok(PreparedIndexWork::Immediate(local_storage)),
                    Err(error) => {
                        let local_storage = incomplete_file_storage(
                            &full_path,
                            None,
                            text_only_language_name(&full_path),
                            codestory_contracts::graph::ErrorInfo {
                                message: format!(
                                    "Failed to index text-only file {:?}: {}",
                                    path, error
                                ),
                                file_id: None,
                                line: None,
                                column: None,
                                is_fatal: false,
                                index_step: codestory_contracts::graph::IndexStep::Indexing,
                                coverage_reason: Some(FileCoverageReason::CollectorFailure),
                            },
                        );
                        Err(local_storage)
                    }
                };
            }
            return match index_inventory_only_file(&full_path, source_language) {
                Ok(local_storage) => Ok(PreparedIndexWork::Immediate(local_storage)),
                Err(error) => Err(incomplete_file_storage(
                    &full_path,
                    None,
                    source_language,
                    codestory_contracts::graph::ErrorInfo {
                        message: format!(
                            "Failed to inventory companion source file {:?}: {}",
                            path, error
                        ),
                        file_id: None,
                        line: None,
                        column: None,
                        is_fatal: false,
                        index_step: codestory_contracts::graph::IndexStep::Collection,
                        coverage_reason: Some(FileCoverageReason::Unreadable),
                    },
                )),
            };
        };

        let bytes = match std::fs::read(&full_path) {
            Ok(bytes) => bytes,
            Err(e) => {
                let local_storage = incomplete_file_storage(
                    &full_path,
                    None,
                    language_config.language_name,
                    codestory_contracts::graph::ErrorInfo {
                        message: format!("Failed to read {:?}: {}", path, e),
                        file_id: None,
                        line: None,
                        column: None,
                        is_fatal: true,
                        index_step: codestory_contracts::graph::IndexStep::Collection,
                        coverage_reason: Some(FileCoverageReason::Unreadable),
                    },
                );
                return Err(local_storage);
            }
        };
        let content_hash = source_content_hash(&bytes);
        let source_utf8_exact = std::str::from_utf8(&bytes).is_ok();
        // Inspect the parser source before building the cache key so source-aware header
        // detection chooses the same language that will be used for indexing. The key remains
        // bound to the raw bytes even when ordinary graph indexing uses a lossy view.
        {
            let parser_source = String::from_utf8_lossy(&bytes);
            if let Some(upgraded) = maybe_upgrade_header_language_from_source(
                &full_path,
                &parser_source,
                &language_config,
            ) {
                language_config = upgraded;
            }
        }
        let flags = index_feature_flags();
        let artifact_cache_path = index_artifact_cache_path(root, &full_path);
        let artifact_cache_key = artifact_cache_path.as_ref().and_then(|cache_path| {
            build_index_artifact_cache_key(
                root,
                cache_path,
                &bytes,
                &language_config,
                compilation_info.as_ref(),
                flags.legacy_edge_identity,
                flags.lazy_graph_execution,
            )
        });
        let source = match String::from_utf8(bytes) {
            Ok(source) => source,
            Err(error) => String::from_utf8_lossy(&error.into_bytes()).into_owned(),
        };
        stats.parser_artifact_cache.record_lookup();

        let Some(cache_path) = artifact_cache_path.as_ref() else {
            stats.artifact_cache_misses += 1;
            stats.parser_artifact_cache.misses += 1;
            return Ok(PreparedIndexWork::Parse(PreparedIndexInput {
                full_path,
                artifact_cache_path,
                source,
                source_utf8_exact,
                compilation_info,
                language_config,
                artifact_cache_key,
                content_hash,
            }));
        };
        let Some(cache_key) = artifact_cache_key.as_ref() else {
            stats.artifact_cache_misses += 1;
            stats.parser_artifact_cache.misses += 1;
            return Ok(PreparedIndexWork::Parse(PreparedIndexInput {
                full_path,
                artifact_cache_path,
                source,
                source_utf8_exact,
                compilation_info,
                language_config,
                artifact_cache_key,
                content_hash,
            }));
        };

        match cache_access.get_parser(cache_path, cache_key, &mut stats.parser_artifact_cache) {
            Ok(Some(blob)) => match serde_json::from_slice::<CachedIndexArtifact>(&blob) {
                Ok(artifact)
                    if proof_resolution::cached_resolution_inputs_are_current(
                        &artifact,
                        language_config.language_name,
                        &resolution_parser_fingerprint(&language_config),
                        &content_hash,
                    ) =>
                {
                    let mut artifact = rebase_cached_index_artifact(
                        artifact,
                        &full_path,
                        &source,
                        language_config.language_name,
                        flags,
                    );
                    verify_cached_artifact_source(
                        &mut artifact,
                        &full_path,
                        language_config.language_name,
                        &content_hash,
                    )?;
                    stats.artifact_cache_hits += 1;
                    stats.parser_artifact_cache.hits += 1;
                    if existing_projection_id.is_some() {
                        let Some(storage) = cache_access.storage_mut() else {
                            let mut local_storage = IntermediateStorage::default();
                            local_storage.add_error(codestory_contracts::graph::ErrorInfo {
                                message: format!(
                                    "Artifact-cache reader cannot refresh an existing projection for {:?}",
                                    full_path
                                ),
                                file_id: artifact.files.first().map(|file| NodeId(file.id)),
                                line: None,
                                column: None,
                                is_fatal: true,
                                index_step: codestory_contracts::graph::IndexStep::Indexing,
                                coverage_reason: Some(FileCoverageReason::CollectorFailure),
                            });
                            return Err(local_storage);
                        };
                        if let Some(file_info) = artifact.files.first()
                            && let Err(error) =
                                storage.update_file_metadata(file_info, Some(&content_hash))
                        {
                            let mut local_storage = IntermediateStorage::default();
                            let file_id = NodeId(file_info.id);
                            local_storage.add_error(codestory_contracts::graph::ErrorInfo {
                                message: format!(
                                    "Failed to refresh cached file metadata for {:?}: {:?}",
                                    full_path, error
                                ),
                                file_id: Some(file_id),
                                line: None,
                                column: None,
                                is_fatal: false,
                                index_step: codestory_contracts::graph::IndexStep::Indexing,
                                coverage_reason: Some(FileCoverageReason::CollectorFailure),
                            });
                            return Err(local_storage);
                        }
                        if let Some(file_info) = artifact.files.first()
                            && let Err(error) =
                                storage.replace_errors_for_files_batch(&[file_info.id], &[])
                        {
                            let mut local_storage = IntermediateStorage::default();
                            local_storage.add_error(codestory_contracts::graph::ErrorInfo {
                                message: format!(
                                    "Failed to replace cached file errors for {:?}: {:?}",
                                    full_path, error
                                ),
                                file_id: Some(NodeId(file_info.id)),
                                line: None,
                                column: None,
                                is_fatal: false,
                                index_step: codestory_contracts::graph::IndexStep::Indexing,
                                coverage_reason: Some(FileCoverageReason::CollectorFailure),
                            });
                            return Err(local_storage);
                        }
                        stats.source_identity_only_files =
                            stats.source_identity_only_files.saturating_add(1);
                        return Ok(PreparedIndexWork::Immediate(IntermediateStorage::default()));
                    }
                    Self::seed_symbol_table_from_nodes(symbol_table, &artifact.nodes);
                    let mut local_storage = artifact.into_intermediate_storage();
                    if let Some(file_info) = local_storage.files.first() {
                        local_storage
                            .file_content_hashes
                            .push(codestory_store::FileContentHash {
                                file_id: file_info.id,
                                content_hash,
                            });
                    }
                    Ok(PreparedIndexWork::Immediate(local_storage))
                }
                Ok(_) | Err(_) => {
                    stats.artifact_cache_invalid_entries += 1;
                    stats.artifact_cache_misses += 1;
                    stats.parser_artifact_cache.misses += 1;
                    Ok(PreparedIndexWork::Parse(PreparedIndexInput {
                        full_path,
                        artifact_cache_path,
                        source,
                        source_utf8_exact,
                        compilation_info,
                        language_config,
                        artifact_cache_key,
                        content_hash,
                    }))
                }
            },
            Ok(None) => {
                stats.artifact_cache_misses += 1;
                stats.parser_artifact_cache.misses += 1;
                Ok(PreparedIndexWork::Parse(PreparedIndexInput {
                    full_path,
                    artifact_cache_path,
                    source,
                    source_utf8_exact,
                    compilation_info,
                    language_config,
                    artifact_cache_key,
                    content_hash,
                }))
            }
            Err(_) => {
                stats.artifact_cache_misses += 1;
                stats.parser_artifact_cache.misses += 1;
                Ok(PreparedIndexWork::Parse(PreparedIndexInput {
                    full_path,
                    artifact_cache_path,
                    source,
                    source_utf8_exact,
                    compilation_info,
                    language_config,
                    artifact_cache_key,
                    content_hash,
                }))
            }
        }
    }

    #[allow(clippy::result_large_err)]
    fn prepare_structural_index_work(
        &self,
        cache_access: &mut ArtifactCacheAccess<'_>,
        path: &Path,
        root: &Path,
        existing_projection_id: Option<i64>,
        stats: &mut IncrementalIndexingStats,
    ) -> std::result::Result<PreparedIndexWork, IntermediateStorage> {
        let full_path = Self::normalize_index_path(root, path);
        let role_classification_path =
            codestory_workspace::workspace_relative_path(root, &full_path)
                .unwrap_or_else(|| path.to_path_buf());
        let language = structural::structural_language_name(&full_path);
        let producer = structural::structural_producer(&full_path)
            .expect("admitted structural paths have one producer");
        let structural_size = std::fs::metadata(&full_path)
            .map_err(|error| {
                incomplete_file_storage(
                    &full_path,
                    None,
                    language,
                    codestory_contracts::graph::ErrorInfo {
                        message: format!("Failed to inspect {:?}: {}", path, error),
                        file_id: None,
                        line: None,
                        column: None,
                        is_fatal: true,
                        index_step: codestory_contracts::graph::IndexStep::Collection,
                        coverage_reason: Some(FileCoverageReason::Unreadable),
                    },
                )
            })?
            .len();
        if structural_size > self.structural_source_byte_cap {
            return Err(incomplete_file_storage(
                &full_path,
                None,
                language,
                codestory_contracts::graph::ErrorInfo {
                    message: format!(
                        "Skipped structural source {:?}: {} bytes exceeds the {} byte structural collector limit",
                        path, structural_size, self.structural_source_byte_cap
                    ),
                    file_id: None,
                    line: None,
                    column: None,
                    is_fatal: false,
                    index_step: codestory_contracts::graph::IndexStep::Indexing,
                    coverage_reason: Some(FileCoverageReason::Oversized),
                },
            ));
        }
        let bytes = std::fs::read(&full_path).map_err(|error| {
            incomplete_file_storage(
                &full_path,
                None,
                language,
                codestory_contracts::graph::ErrorInfo {
                    message: format!("Failed to read {:?}: {}", path, error),
                    file_id: None,
                    line: None,
                    column: None,
                    is_fatal: true,
                    index_step: codestory_contracts::graph::IndexStep::Collection,
                    coverage_reason: Some(FileCoverageReason::Unreadable),
                },
            )
        })?;
        let content_hash = source_content_hash(&bytes);
        let source = structural::decode_structural_source(bytes).map_err(|error| {
            incomplete_file_storage(
                &full_path,
                None,
                language,
                codestory_contracts::graph::ErrorInfo {
                    message: format!("Failed to index structural file {:?}: {}", path, error),
                    file_id: None,
                    line: None,
                    column: None,
                    is_fatal: false,
                    index_step: codestory_contracts::graph::IndexStep::Indexing,
                    coverage_reason: Some(FileCoverageReason::Binary),
                },
            )
        })?;
        let artifact_cache_path = index_artifact_cache_path(root, &full_path);
        let artifact_cache_key = artifact_cache_path.as_ref().and_then(|cache_path| {
            build_structural_artifact_cache_key(cache_path, source.as_bytes(), producer)
        });
        stats.structural_artifact_cache.record_lookup();
        let prepared_input = || PreparedStructuralInput {
            full_path: full_path.clone(),
            role_classification_path: role_classification_path.clone(),
            artifact_cache_path: artifact_cache_path.clone(),
            artifact_cache_key: artifact_cache_key.clone(),
            source: source.clone(),
            content_hash: content_hash.clone(),
        };
        let Some(cache_path) = artifact_cache_path.as_ref() else {
            stats.artifact_cache_misses += 1;
            stats.structural_artifact_cache.misses += 1;
            return Ok(PreparedIndexWork::Structural(prepared_input()));
        };
        let Some(cache_key) = artifact_cache_key.as_ref() else {
            stats.artifact_cache_misses += 1;
            stats.structural_artifact_cache.misses += 1;
            return Ok(PreparedIndexWork::Structural(prepared_input()));
        };

        let cached = cache_access.get_structural(
            cache_path,
            cache_key,
            &mut stats.structural_artifact_cache,
        );
        let Ok(Some(blob)) = cached else {
            stats.artifact_cache_misses += 1;
            stats.structural_artifact_cache.misses += 1;
            return Ok(PreparedIndexWork::Structural(prepared_input()));
        };
        let Ok(mut artifact) = serde_json::from_slice::<CachedStructuralArtifact>(&blob) else {
            stats.artifact_cache_invalid_entries += 1;
            stats.artifact_cache_misses += 1;
            stats.structural_artifact_cache.misses += 1;
            return Ok(PreparedIndexWork::Structural(prepared_input()));
        };
        let structural_unit_cap = self
            .source_index_policy
            .as_ref()
            .map(|policy| policy.structural_unit_cap)
            .unwrap_or(codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP);
        if artifact.descriptor_version != codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION
            || artifact.files.first().map(|file| file.id)
                != Some(Self::canonical_file_node_id_for_path(&full_path))
            || artifact.structural_unit_node_ids.len() as u64 > structural_unit_cap
            || artifact.structural_text_units.len() as u64 > structural_unit_cap
            || artifact
                .structural_text_projections
                .iter()
                .any(|projection| projection.unit_count > structural_unit_cap)
        {
            stats.artifact_cache_invalid_entries += 1;
            stats.artifact_cache_misses += 1;
            stats.structural_artifact_cache.misses += 1;
            return Ok(PreparedIndexWork::Structural(prepared_input()));
        }
        let modification_time = verify_source_snapshot(&full_path, &content_hash)
            .map_err(|error| changed_source_storage(&full_path, language, error))?;
        if let Some(file_info) = artifact.files.first_mut() {
            file_info.modification_time = modification_time;
            file_info.path = full_path.clone();
            file_info.language = language.to_string();
            file_info.line_count = source.lines().count() as u32;
        }
        let expected_storage = structural::finalize_structural_storage(
            &full_path,
            &source,
            &content_hash,
            artifact.clone().into_intermediate_storage(),
        )
        .map_err(|error| {
            incomplete_file_storage(
                &full_path,
                Some(&source),
                language,
                codestory_contracts::graph::ErrorInfo {
                    message: format!(
                        "Failed to validate cached structural file {:?}: {}",
                        path, error
                    ),
                    file_id: None,
                    line: None,
                    column: None,
                    is_fatal: false,
                    index_step: codestory_contracts::graph::IndexStep::Indexing,
                    coverage_reason: Some(FileCoverageReason::CollectorFailure),
                },
            )
        })?;
        if expected_storage.file_content_hashes != artifact.file_content_hashes
            || expected_storage.structural_text_units != artifact.structural_text_units
            || expected_storage.structural_text_projections != artifact.structural_text_projections
            || expected_storage.structural_unit_node_ids != artifact.structural_unit_node_ids
        {
            stats.artifact_cache_invalid_entries += 1;
            stats.artifact_cache_misses += 1;
            stats.structural_artifact_cache.misses += 1;
            return Ok(PreparedIndexWork::Structural(prepared_input()));
        }
        stats.artifact_cache_hits += 1;
        stats.structural_artifact_cache.hits += 1;
        if existing_projection_id.is_some() {
            let Some(storage) = cache_access.storage_mut() else {
                return Err(incomplete_file_storage(
                    &full_path,
                    Some(&source),
                    language,
                    codestory_contracts::graph::ErrorInfo {
                        message: format!(
                            "Artifact-cache reader cannot refresh an existing structural projection for {:?}",
                            full_path
                        ),
                        file_id: None,
                        line: None,
                        column: None,
                        is_fatal: true,
                        index_step: codestory_contracts::graph::IndexStep::Indexing,
                        coverage_reason: Some(FileCoverageReason::CollectorFailure),
                    },
                ));
            };
            if let Some(file_info) = expected_storage.files.first() {
                storage
                    .update_file_metadata(file_info, Some(&content_hash))
                    .map_err(|error| {
                        incomplete_file_storage(
                            &full_path,
                            Some(&source),
                            language,
                            codestory_contracts::graph::ErrorInfo {
                                message: format!(
                                    "Failed to refresh cached structural metadata for {:?}: {:?}",
                                    full_path, error
                                ),
                                file_id: None,
                                line: None,
                                column: None,
                                is_fatal: false,
                                index_step: codestory_contracts::graph::IndexStep::Indexing,
                                coverage_reason: Some(FileCoverageReason::CollectorFailure),
                            },
                        )
                    })?;
                storage
                    .replace_errors_for_files_batch(&[file_info.id], &[])
                    .map_err(|error| {
                        incomplete_file_storage(
                            &full_path,
                            Some(&source),
                            language,
                            codestory_contracts::graph::ErrorInfo {
                                message: format!(
                                    "Failed to clear cached structural errors for {:?}: {:?}",
                                    full_path, error
                                ),
                                file_id: None,
                                line: None,
                                column: None,
                                is_fatal: false,
                                index_step: codestory_contracts::graph::IndexStep::Indexing,
                                coverage_reason: Some(FileCoverageReason::CollectorFailure),
                            },
                        )
                    })?;
            }
            return Ok(PreparedIndexWork::Immediate(IntermediateStorage::default()));
        }
        Ok(PreparedIndexWork::Immediate(expected_storage))
    }

    #[allow(clippy::result_large_err)]
    fn prepare_openapi_schema_work(
        &self,
        full_path: &Path,
    ) -> std::result::Result<Option<IntermediateStorage>, IntermediateStorage> {
        let path_text = full_path.to_string_lossy();
        if codestory_contracts::language_support::is_github_actions_workflow_path(
            path_text.as_ref(),
        ) || codestory_contracts::language_support::is_docker_compose_file_path(
            path_text.as_ref(),
        ) || codestory_contracts::language_support::is_typescript_config_jsonc_file_path(
            path_text.as_ref(),
        ) {
            return Ok(None);
        }
        if !is_openapi_candidate_path(full_path) {
            return Ok(None);
        }
        // OpenAPI candidates are `.json`/`.yaml`/`.yml`, so planning already
        // excludes them above the structural cap and this only catches a file
        // that grew since — the same growth race `:2919` covers for parsers.
        // Without it this is the widest unbounded read in the crate:
        // `decode_structural_source` has no size check of its own, so the whole
        // file is read and projected on the strength of the caller's bound.
        match std::fs::metadata(full_path) {
            Ok(metadata) if metadata.len() > self.structural_source_byte_cap => {
                return Err(incomplete_file_storage(
                    full_path,
                    None,
                    "openapi",
                    codestory_contracts::graph::ErrorInfo {
                        message: format!(
                            "Skipped OpenAPI schema {:?}: {} bytes exceeds the {} byte structural collector limit",
                            full_path,
                            metadata.len(),
                            self.structural_source_byte_cap
                        ),
                        file_id: None,
                        line: None,
                        column: None,
                        is_fatal: false,
                        index_step: codestory_contracts::graph::IndexStep::Indexing,
                        coverage_reason: Some(FileCoverageReason::Oversized),
                    },
                ));
            }
            _ => {}
        }
        let bytes = match std::fs::read(full_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                let local_storage = incomplete_file_storage(
                    full_path,
                    None,
                    "openapi",
                    codestory_contracts::graph::ErrorInfo {
                        message: format!("Failed to read {:?}: {}", full_path, error),
                        file_id: None,
                        line: None,
                        column: None,
                        is_fatal: true,
                        index_step: codestory_contracts::graph::IndexStep::Collection,
                        coverage_reason: Some(FileCoverageReason::Unreadable),
                    },
                );
                return Err(local_storage);
            }
        };
        let content_hash = source_content_hash(&bytes);
        let source = match structural::decode_structural_source(bytes) {
            Ok(source) => source,
            Err(_) if structural::is_structural_candidate_path(full_path) => return Ok(None),
            Err(error) => {
                return Err(incomplete_file_storage(
                    full_path,
                    None,
                    "openapi",
                    codestory_contracts::graph::ErrorInfo {
                        message: format!("Failed to decode {:?}: {}", full_path, error),
                        file_id: None,
                        line: None,
                        column: None,
                        is_fatal: false,
                        index_step: codestory_contracts::graph::IndexStep::Collection,
                        coverage_reason: Some(FileCoverageReason::Binary),
                    },
                ));
            }
        };
        let mut projected = index_openapi_schema_file(full_path, &source).map_err(|error| {
            incomplete_file_storage(
                full_path,
                Some(&source),
                "openapi",
                codestory_contracts::graph::ErrorInfo {
                    message: format!("Failed to index OpenAPI schema {:?}: {}", full_path, error),
                    file_id: None,
                    line: None,
                    column: None,
                    is_fatal: false,
                    index_step: codestory_contracts::graph::IndexStep::Indexing,
                    coverage_reason: Some(FileCoverageReason::CollectorFailure),
                },
            )
        })?;
        if let Some(projected) = projected.as_mut() {
            let modification_time = verify_source_snapshot(full_path, &content_hash)
                .map_err(|error| changed_source_storage(full_path, "openapi", error))?;
            if let Some(file) = projected.files.first_mut() {
                file.modification_time = modification_time;
                projected
                    .file_content_hashes
                    .push(codestory_store::FileContentHash {
                        file_id: file.id,
                        content_hash,
                    });
            }
        }
        Ok(projected)
    }

    fn execute_prepared_index(
        &self,
        prepared_input: &PreparedIndexInput,
        symbol_table: &Arc<SymbolTable>,
    ) -> PreparedIndexJobResult {
        let index_result = index_file_with_resolution_inputs(
            &prepared_input.full_path,
            &prepared_input.source,
            &prepared_input.content_hash,
            &prepared_input.language_config,
            prepared_input.compilation_info.clone(),
            Some(Arc::clone(symbol_table)),
        );
        let modification_time =
            match verify_source_snapshot(&prepared_input.full_path, &prepared_input.content_hash) {
                Ok(modification_time) => modification_time,
                Err(error) => {
                    return PreparedIndexJobResult {
                        local_storage: changed_source_storage(
                            &prepared_input.full_path,
                            prepared_input.language_config.language_name,
                            error,
                        ),
                        cache_write: None,
                        policy_exclusion: None,
                    };
                }
            };

        match index_result {
            Ok((mut index_result, mut call_resolution_inputs, mut resolution_file)) => {
                if let Some(file_info) = index_result.files.first_mut() {
                    file_info.modification_time = modification_time;
                }
                if !prepared_input.source_utf8_exact {
                    call_resolution_inputs.clear();
                    if let Some(file) = resolution_file.as_mut() {
                        file.source_sha256 = prepared_input.content_hash.clone();
                        file.lookup_input_complete = false;
                    }
                }
                let artifact = CachedIndexArtifact::from_index_result_with_resolution_inputs(
                    index_result,
                    call_resolution_inputs,
                    resolution_file,
                );
                let cache_write = prepared_input
                    .artifact_cache_path
                    .as_ref()
                    .zip(prepared_input.artifact_cache_key.as_ref())
                    .and_then(|(path, cache_key)| {
                        serde_json::to_vec(&artifact)
                            .ok()
                            .map(|artifact_blob| ArtifactCacheWrite {
                                path: path.clone(),
                                cache_key: cache_key.clone(),
                                artifact_blob,
                            })
                    });
                let mut local_storage = artifact.into_intermediate_storage();
                if let Some(file_info) = local_storage.files.first() {
                    local_storage
                        .file_content_hashes
                        .push(codestory_store::FileContentHash {
                            file_id: file_info.id,
                            content_hash: prepared_input.content_hash.clone(),
                        });
                }
                PreparedIndexJobResult {
                    local_storage,
                    cache_write,
                    policy_exclusion: None,
                }
            }
            Err(e) => {
                let mut local_storage = incomplete_file_storage(
                    &prepared_input.full_path,
                    Some(&prepared_input.source),
                    prepared_input.language_config.language_name,
                    codestory_contracts::graph::ErrorInfo {
                        message: format!("Failed to index {:?}: {}", prepared_input.full_path, e),
                        file_id: None,
                        line: None,
                        column: None,
                        is_fatal: false,
                        index_step: codestory_contracts::graph::IndexStep::Indexing,
                        coverage_reason: Some(FileCoverageReason::CollectorFailure),
                    },
                );
                if let Some(file_info) = local_storage.files.first_mut() {
                    file_info.modification_time = modification_time;
                    local_storage
                        .file_content_hashes
                        .push(codestory_store::FileContentHash {
                            file_id: file_info.id,
                            content_hash: prepared_input.content_hash.clone(),
                        });
                }
                PreparedIndexJobResult {
                    local_storage,
                    cache_write: None,
                    policy_exclusion: None,
                }
            }
        }
    }

    fn execute_prepared_structural_index(
        &self,
        prepared_input: &PreparedStructuralInput,
    ) -> PreparedIndexJobResult {
        let language = structural::structural_language_name(&prepared_input.full_path);
        let structural_unit_cap = self
            .source_index_policy
            .as_ref()
            .map(|policy| policy.structural_unit_cap)
            .unwrap_or(codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP);
        let collected = match structural::index_structural_source_with_role_and_unit_cap(
            &prepared_input.full_path,
            &prepared_input.role_classification_path,
            &prepared_input.source,
            structural_unit_cap,
            self.structural_source_byte_cap,
        ) {
            Ok(collected) => collected,
            Err(structural::StructuralCollectionError::UnitLimit {
                observed_unit_count,
                structural_unit_cap: observed_cap,
            }) if self.source_index_policy.is_some() => {
                let policy = self
                    .source_index_policy
                    .as_ref()
                    .expect("guarded source policy");
                debug_assert_eq!(observed_cap, policy.structural_unit_cap);
                let verified =
                    verify_source_snapshot(&prepared_input.full_path, &prepared_input.content_hash);
                if let Err(error) = verified {
                    return PreparedIndexJobResult {
                        local_storage: changed_source_storage(
                            &prepared_input.full_path,
                            language,
                            error,
                        ),
                        cache_write: None,
                        policy_exclusion: None,
                    };
                }
                let Some(relative) = codestory_workspace::workspace_relative_path(
                    &self.root,
                    &prepared_input.full_path,
                ) else {
                    return PreparedIndexJobResult {
                        local_storage: incomplete_file_storage(
                            &prepared_input.full_path,
                            Some(&prepared_input.source),
                            language,
                            codestory_contracts::graph::ErrorInfo {
                                message: "Failed to bind structural policy exclusion to the workspace root".into(),
                                file_id: None,
                                line: None,
                                column: None,
                                is_fatal: false,
                                index_step: codestory_contracts::graph::IndexStep::Indexing,
                                coverage_reason: Some(FileCoverageReason::CollectorFailure),
                            },
                        ),
                        cache_write: None,
                        policy_exclusion: None,
                    };
                };
                let normalized_path = relative.to_string_lossy().replace('\\', "/");
                let effective_byte_cap = policy.effective_byte_cap(&normalized_path);
                return PreparedIndexJobResult {
                    local_storage: IntermediateStorage::default(),
                    cache_write: None,
                    policy_exclusion: Some(PreparedPolicyExclusion {
                        file_id: Self::canonical_file_node_id_for_path(&prepared_input.full_path),
                        candidate: OversizedSourceExclusionCandidate {
                            normalized_path,
                            content_hash: prepared_input.content_hash.clone(),
                            observed_size: prepared_input.source.len() as u64,
                            observed_unit_count,
                            policy_version: policy.policy_version.clone(),
                            // The cap that governs this path, not the parser
                            // headroom. This is a structural source by
                            // construction, so it is the structural bound, and
                            // revalidation at the publication fence recomputes
                            // exactly the same value from the same path.
                            byte_cap: effective_byte_cap,
                            structural_unit_cap: policy.structural_unit_cap,
                        },
                    }),
                };
            }
            Err(error) => {
                let reason = match error {
                    structural::StructuralCollectionError::Malformed(_) => {
                        FileCoverageReason::Malformed
                    }
                    structural::StructuralCollectionError::Binary => FileCoverageReason::Binary,
                    structural::StructuralCollectionError::SourceByteLimit { .. }
                    | structural::StructuralCollectionError::UnitLimit { .. } => {
                        FileCoverageReason::Oversized
                    }
                };
                let mut local_storage = incomplete_file_storage(
                    &prepared_input.full_path,
                    Some(&prepared_input.source),
                    language,
                    codestory_contracts::graph::ErrorInfo {
                        message: format!(
                            "Failed to index structural file {:?}: {}",
                            prepared_input.full_path, error
                        ),
                        file_id: None,
                        line: None,
                        column: None,
                        is_fatal: false,
                        index_step: codestory_contracts::graph::IndexStep::Indexing,
                        coverage_reason: Some(reason),
                    },
                );
                if reason == FileCoverageReason::Malformed {
                    match verify_source_snapshot(
                        &prepared_input.full_path,
                        &prepared_input.content_hash,
                    ) {
                        Ok(modification_time) => {
                            let file = &mut local_storage.files[0];
                            file.modification_time = modification_time;
                            local_storage.file_content_hashes.push(
                                codestory_store::FileContentHash {
                                    file_id: file.id,
                                    content_hash: prepared_input.content_hash.clone(),
                                },
                            );
                        }
                        Err(error) => {
                            local_storage =
                                changed_source_storage(&prepared_input.full_path, language, error);
                        }
                    }
                }
                return PreparedIndexJobResult {
                    local_storage,
                    cache_write: None,
                    policy_exclusion: None,
                };
            }
        };
        let modification_time =
            match verify_source_snapshot(&prepared_input.full_path, &prepared_input.content_hash) {
                Ok(modification_time) => modification_time,
                Err(error) => {
                    return PreparedIndexJobResult {
                        local_storage: changed_source_storage(
                            &prepared_input.full_path,
                            language,
                            error,
                        ),
                        cache_write: None,
                        policy_exclusion: None,
                    };
                }
            };
        let mut local_storage = match structural::finalize_structural_storage(
            &prepared_input.full_path,
            &prepared_input.source,
            &prepared_input.content_hash,
            collected,
        ) {
            Ok(storage) => storage,
            Err(error) => {
                return PreparedIndexJobResult {
                    local_storage: incomplete_file_storage(
                        &prepared_input.full_path,
                        Some(&prepared_input.source),
                        language,
                        codestory_contracts::graph::ErrorInfo {
                            message: format!(
                                "Failed to index structural file {:?}: {}",
                                prepared_input.full_path, error
                            ),
                            file_id: None,
                            line: None,
                            column: None,
                            is_fatal: false,
                            index_step: codestory_contracts::graph::IndexStep::Indexing,
                            coverage_reason: Some(FileCoverageReason::CollectorFailure),
                        },
                    ),
                    cache_write: None,
                    policy_exclusion: None,
                };
            }
        };
        if let Some(file_info) = local_storage.files.first_mut() {
            file_info.modification_time = modification_time;
        }
        let structural_file_id = local_storage.files.first().map(|file| file.id);
        let artifact = CachedStructuralArtifact::from_storage(local_storage);
        let structural_cache_write = prepared_input
            .artifact_cache_path
            .as_ref()
            .zip(prepared_input.artifact_cache_key.as_ref())
            .zip(structural_file_id)
            .and_then(|((path, cache_key), file_id)| {
                serde_json::to_vec(&artifact)
                    .ok()
                    .map(|artifact_blob| (file_id, path.clone(), cache_key.clone(), artifact_blob))
            });
        local_storage = artifact.into_intermediate_storage();
        if let Some((file_id, path, cache_key, artifact_blob)) = structural_cache_write {
            local_storage.structural_text_cache_writes.push(
                intermediate_storage::StructuralTextArtifactCacheWrite {
                    path,
                    file_id,
                    cache_key,
                    artifact_blob,
                },
            );
        }
        PreparedIndexJobResult {
            local_storage,
            cache_write: None,
            policy_exclusion: None,
        }
    }

    fn seed_symbol_table_from_nodes(symbol_table: &SymbolTable, nodes: &[Node]) {
        for node in nodes {
            symbol_table.insert(node.id.0, node.kind);
        }
    }
}

fn source_content_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn resolution_parser_fingerprint(language_config: &LanguageConfig) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codestory-proof-parser-rules-v4\0");
    hasher.update(language_config.language_name.as_bytes());
    hasher.update(language_config.language.abi_version().to_be_bytes());
    hasher.update(language_config.graph_query.as_bytes());
    if let Some(tags_query) = language_config.tags_query {
        hasher.update(tags_query.as_bytes());
    }
    for id in 0..language_config.language.node_kind_count() {
        hasher.update((id as u64).to_be_bytes());
        if let Some(kind) = language_config.language.node_kind_for_id(id as u16) {
            hasher.update(kind.as_bytes());
        }
        hasher.update([
            u8::from(language_config.language.node_kind_is_named(id as u16)),
            u8::from(language_config.language.node_kind_is_visible(id as u16)),
        ]);
    }
    format!("{:x}", hasher.finalize())
}

fn verify_source_snapshot(path: &Path, expected_hash: &str) -> Result<i64> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| anyhow!("failed to re-open {}: {error}", path.display()))?;
    let before = file
        .metadata()
        .map_err(|error| anyhow!("failed to inspect {}: {error}", path.display()))?;
    let mut bytes = Vec::with_capacity(before.len() as usize);
    std::io::Read::read_to_end(&mut file, &mut bytes)
        .map_err(|error| anyhow!("failed to re-read {}: {error}", path.display()))?;
    let after = file
        .metadata()
        .map_err(|error| anyhow!("failed to re-inspect {}: {error}", path.display()))?;
    if before.len() != after.len() || before.modified()? != after.modified()? {
        return Err(anyhow!("metadata changed during the verification read"));
    }
    let actual_hash = source_content_hash(&bytes);
    if actual_hash != expected_hash {
        return Err(anyhow!("content changed after the indexing read"));
    }
    Ok(codestory_workspace::clamp_system_time_to_epoch_millis(
        after.modified()?,
    ))
}

fn workspace_structural_source_exclusion(
    workspace_root: &Path,
    path: &Path,
) -> Option<&'static str> {
    if !structural::is_structural_format_path(path) {
        return None;
    }
    let relative = codestory_workspace::workspace_relative_path(workspace_root, path)?;
    let relative = relative.to_str()?.replace('\\', "/");
    codestory_contracts::language_support::structural_source_path_exclusion(&relative)
}

#[allow(clippy::result_large_err)]
fn verify_cached_artifact_source(
    artifact: &mut CachedIndexArtifact,
    path: &Path,
    language: &str,
    expected_hash: &str,
) -> std::result::Result<(), IntermediateStorage> {
    let modification_time = verify_source_snapshot(path, expected_hash)
        .map_err(|error| changed_source_storage(path, language, error))?;
    if let Some(file_info) = artifact.files.first_mut() {
        file_info.modification_time = modification_time;
    }
    Ok(())
}

fn changed_source_storage(
    path: &Path,
    language: impl Into<String>,
    error: anyhow::Error,
) -> IntermediateStorage {
    incomplete_file_storage(
        path,
        None,
        language,
        codestory_contracts::graph::ErrorInfo {
            message: format!(
                "Source changed while indexing {}; retry required: {error}",
                path.display()
            ),
            file_id: None,
            line: None,
            column: None,
            is_fatal: false,
            index_step: codestory_contracts::graph::IndexStep::Indexing,
            coverage_reason: Some(FileCoverageReason::SourceChanged),
        },
    )
}

fn incremental_resolution_target_node_kinds() -> &'static [NodeKind] {
    &[
        NodeKind::MODULE,
        NodeKind::NAMESPACE,
        NodeKind::PACKAGE,
        NodeKind::STRUCT,
        NodeKind::CLASS,
        NodeKind::INTERFACE,
        NodeKind::ANNOTATION,
        NodeKind::UNION,
        NodeKind::ENUM,
        NodeKind::TYPEDEF,
        NodeKind::FUNCTION,
        NodeKind::METHOD,
        NodeKind::MACRO,
        NodeKind::GLOBAL_VARIABLE,
        NodeKind::FIELD,
        NodeKind::CONSTANT,
        NodeKind::ENUM_CONSTANT,
    ]
}

fn file_modification_time(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(codestory_workspace::clamp_system_time_to_epoch_millis)
        .unwrap_or(0)
}

fn duration_ms_u64(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

fn projection_batch_has_rows(storage: &IntermediateStorage) -> bool {
    !storage.files.is_empty()
        || !storage.file_content_hashes.is_empty()
        || !storage.nodes.is_empty()
        || !storage.structural_text_units.is_empty()
        || !storage.structural_text_projections.is_empty()
        || !storage.structural_text_cache_writes.is_empty()
        || !storage.edges.is_empty()
        || !storage.occurrences.is_empty()
        || !storage.component_access.is_empty()
        || !storage.callable_projection_states.is_empty()
}

fn projection_flush_breakdown_ms(breakdown: &codestory_store::ProjectionFlushBreakdown) -> u64 {
    u64::from(breakdown.files_ms)
        .saturating_add(u64::from(breakdown.nodes_ms))
        .saturating_add(u64::from(breakdown.structural_text_units_ms))
        .saturating_add(u64::from(breakdown.edges_ms))
        .saturating_add(u64::from(breakdown.occurrences_ms))
        .saturating_add(u64::from(breakdown.component_access_ms))
        .saturating_add(u64::from(breakdown.callable_projection_ms))
}

fn accumulate_projection_writer_stats(
    stats: &mut IncrementalIndexingStats,
    writer_stats: &IncrementalIndexingStats,
) {
    stats.graph_projection_changed |= writer_stats.graph_projection_changed;
    stats.source_identity_only_files = stats
        .source_identity_only_files
        .saturating_add(writer_stats.source_identity_only_files);
    stats.artifact_cache_write_ms = stats
        .artifact_cache_write_ms
        .saturating_add(writer_stats.artifact_cache_write_ms);
    stats.artifact_cache_writes = stats
        .artifact_cache_writes
        .saturating_add(writer_stats.artifact_cache_writes);
    stats.artifact_cache_write_transactions = stats
        .artifact_cache_write_transactions
        .saturating_add(writer_stats.artifact_cache_write_transactions);
    stats.full_refresh_chunks_persisted = stats
        .full_refresh_chunks_persisted
        .saturating_add(writer_stats.full_refresh_chunks_persisted);
    stats.full_refresh_writer_idle_ms = stats
        .full_refresh_writer_idle_ms
        .saturating_add(writer_stats.full_refresh_writer_idle_ms);
    stats.projection_flush_ms = stats
        .projection_flush_ms
        .saturating_add(writer_stats.projection_flush_ms);
    stats.projection_batch_wall_ms = stats
        .projection_batch_wall_ms
        .saturating_add(writer_stats.projection_batch_wall_ms);
    stats.projection_batch_transactions = stats
        .projection_batch_transactions
        .saturating_add(writer_stats.projection_batch_transactions);
    stats
        .projection_persistence
        .accumulate(writer_stats.projection_persistence);
    stats.flush_files_ms = stats
        .flush_files_ms
        .saturating_add(writer_stats.flush_files_ms);
    stats.flush_nodes_ms = stats
        .flush_nodes_ms
        .saturating_add(writer_stats.flush_nodes_ms);
    stats.flush_structural_text_units_ms = stats
        .flush_structural_text_units_ms
        .saturating_add(writer_stats.flush_structural_text_units_ms);
    stats.flush_edges_ms = stats
        .flush_edges_ms
        .saturating_add(writer_stats.flush_edges_ms);
    stats.flush_occurrences_ms = stats
        .flush_occurrences_ms
        .saturating_add(writer_stats.flush_occurrences_ms);
    stats.flush_component_access_ms = stats
        .flush_component_access_ms
        .saturating_add(writer_stats.flush_component_access_ms);
    stats.flush_callable_projection_ms = stats
        .flush_callable_projection_ms
        .saturating_add(writer_stats.flush_callable_projection_ms);
    stats.error_flush_ms = stats
        .error_flush_ms
        .saturating_add(writer_stats.error_flush_ms);
    stats.cleanup_ms = stats.cleanup_ms.saturating_add(writer_stats.cleanup_ms);
}

fn accumulate_flush_breakdown(
    stats: &mut IncrementalIndexingStats,
    breakdown: codestory_store::ProjectionFlushBreakdown,
) {
    stats
        .projection_persistence
        .accumulate(breakdown.persistence);
    let total = projection_flush_breakdown_ms(&breakdown);
    stats.projection_flush_ms = stats.projection_flush_ms.saturating_add(total);
    stats.flush_files_ms = stats
        .flush_files_ms
        .saturating_add(u64::from(breakdown.files_ms));
    stats.flush_nodes_ms = stats
        .flush_nodes_ms
        .saturating_add(u64::from(breakdown.nodes_ms));
    stats.flush_structural_text_units_ms = stats
        .flush_structural_text_units_ms
        .saturating_add(u64::from(breakdown.structural_text_units_ms));
    stats.flush_edges_ms = stats
        .flush_edges_ms
        .saturating_add(u64::from(breakdown.edges_ms));
    stats.flush_occurrences_ms = stats
        .flush_occurrences_ms
        .saturating_add(u64::from(breakdown.occurrences_ms));
    stats.flush_component_access_ms = stats
        .flush_component_access_ms
        .saturating_add(u64::from(breakdown.component_access_ms));
    stats.flush_callable_projection_ms = stats
        .flush_callable_projection_ms
        .saturating_add(u64::from(breakdown.callable_projection_ms));
}

pub(crate) fn file_node_from_source(path: &Path, source: &str) -> (Node, String, NodeId) {
    let file_name = path.to_string_lossy().to_string();
    let file_identity = WorkspaceIndexer::file_identity_path(path);
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

    (file_node, file_identity, file_id)
}

fn rebase_cached_index_artifact(
    mut artifact: CachedIndexArtifact,
    full_path: &Path,
    source: &str,
    language_name: &str,
    flags: IndexFeatureFlags,
) -> CachedIndexArtifact {
    let file_name = full_path.to_string_lossy().to_string();
    let file_identity = WorkspaceIndexer::file_identity_path(full_path);
    for node in &mut artifact.nodes {
        if node.kind == NodeKind::FILE {
            node.serialized_name = file_name.clone();
            node.qualified_name = None;
            node.canonical_id = None;
        }
    }

    let old_file_id = artifact.files.first().map(|file| NodeId(file.id));
    let (nodes, id_remap) = canonicalize_nodes(&file_identity, artifact.nodes, &HashMap::new());
    let fallback_file_id = NodeId(WorkspaceIndexer::canonical_file_node_id_for_path(full_path));
    let new_file_id = old_file_id
        .and_then(|file_id| id_remap.get(&file_id).copied())
        .unwrap_or(fallback_file_id);
    let final_node_ids = nodes.iter().map(|node| node.id).collect::<HashSet<_>>();

    artifact.nodes = nodes;
    remap_file_affinity(&mut artifact.nodes, new_file_id);
    remap_edges(&mut artifact.edges, new_file_id, &id_remap, flags);
    remap_occurrences(&mut artifact.occurrences, &id_remap);
    artifact.component_access = artifact
        .component_access
        .into_iter()
        .filter_map(|(node_id, access)| {
            let remapped = id_remap.get(&node_id).copied().unwrap_or(node_id);
            final_node_ids
                .contains(&remapped)
                .then_some((remapped, access))
        })
        .collect();
    artifact.impl_anchor_node_ids = artifact
        .impl_anchor_node_ids
        .into_iter()
        .map(|node_id| id_remap.get(&node_id).copied().unwrap_or(node_id))
        .filter(|node_id| final_node_ids.contains(node_id))
        .collect();
    artifact.impl_anchor_node_ids.sort_unstable();
    artifact.impl_anchor_node_ids.dedup();
    for input in &mut artifact.call_resolution_inputs {
        input.callsite.file_id = codestory_contracts::proof_resolution::FileId(new_file_id.0);
        input.caller = input
            .caller
            .map(|caller| id_remap.get(&caller).copied().unwrap_or(caller));
        use cache::CachedResolutionBinding;
        input.binding = match input.binding.clone() {
            CachedResolutionBinding::SameFile {
                declaration,
                rust_glob_local_module,
            } => CachedResolutionBinding::SameFile {
                declaration: id_remap.get(&declaration).copied().unwrap_or(declaration),
                rust_glob_local_module,
            },
            CachedResolutionBinding::StaticImport {
                import,
                module_specifier,
                imported_name,
                is_default,
            } => CachedResolutionBinding::StaticImport {
                import: id_remap.get(&import).copied().unwrap_or(import),
                module_specifier,
                imported_name,
                is_default,
            },
            CachedResolutionBinding::ImplicitReceiver {
                owner,
                declaration,
                owner_name,
            } => CachedResolutionBinding::ImplicitReceiver {
                owner: id_remap.get(&owner).copied().unwrap_or(owner),
                declaration: id_remap.get(&declaration).copied().unwrap_or(declaration),
                owner_name,
            },
            CachedResolutionBinding::ConstructorBinding {
                class_binding,
                method_name,
            } => CachedResolutionBinding::ConstructorBinding {
                class_binding: match class_binding {
                    cache::CachedClassBinding::SameFile { owner, owner_name } => {
                        cache::CachedClassBinding::SameFile {
                            owner: id_remap.get(&owner).copied().unwrap_or(owner),
                            owner_name,
                        }
                    }
                    cache::CachedClassBinding::StaticImport {
                        import,
                        module_specifier,
                        imported_name,
                        is_default,
                    } => cache::CachedClassBinding::StaticImport {
                        import: id_remap.get(&import).copied().unwrap_or(import),
                        module_specifier,
                        imported_name,
                        is_default,
                    },
                },
                method_name,
            },
            CachedResolutionBinding::ExplicitReceiverType {
                class_binding,
                method_name,
            } => CachedResolutionBinding::ExplicitReceiverType {
                class_binding: match class_binding {
                    cache::CachedClassBinding::SameFile { owner, owner_name } => {
                        cache::CachedClassBinding::SameFile {
                            owner: id_remap.get(&owner).copied().unwrap_or(owner),
                            owner_name,
                        }
                    }
                    cache::CachedClassBinding::StaticImport {
                        import,
                        module_specifier,
                        imported_name,
                        is_default,
                    } => cache::CachedClassBinding::StaticImport {
                        import: id_remap.get(&import).copied().unwrap_or(import),
                        module_specifier,
                        imported_name,
                        is_default,
                    },
                },
                method_name,
            },
            CachedResolutionBinding::RustPath {
                module_path,
                components,
                import,
                associated_owner,
            } => CachedResolutionBinding::RustPath {
                module_path,
                components,
                import: import.map(|mut import| {
                    import.import = id_remap
                        .get(&import.import)
                        .copied()
                        .unwrap_or(import.import);
                    import
                }),
                associated_owner: associated_owner
                    .map(|owner| id_remap.get(&owner).copied().unwrap_or(owner)),
            },
            CachedResolutionBinding::RustImplicitReceiver {
                module_path,
                owner_name,
                mut import,
                declaration,
            } => {
                import.import = id_remap
                    .get(&import.import)
                    .copied()
                    .unwrap_or(import.import);
                CachedResolutionBinding::RustImplicitReceiver {
                    module_path,
                    owner_name,
                    import,
                    declaration: id_remap.get(&declaration).copied().unwrap_or(declaration),
                }
            }
            CachedResolutionBinding::RustExplicitReceiver {
                module_path,
                owner_name,
                import,
                constructor,
                constructor_record,
                constructor_method,
            } => CachedResolutionBinding::RustExplicitReceiver {
                module_path,
                owner_name,
                import: import.map(|mut import| {
                    import.import = id_remap
                        .get(&import.import)
                        .copied()
                        .unwrap_or(import.import);
                    import
                }),
                constructor,
                constructor_record,
                constructor_method,
            },
            CachedResolutionBinding::CCppQualified { components } => {
                CachedResolutionBinding::CCppQualified {
                    components: components
                        .into_iter()
                        .map(|component| id_remap.get(&component).copied().unwrap_or(component))
                        .collect(),
                }
            }
            other => other,
        };
    }
    if let Some(resolution_file) = &mut artifact.resolution_file {
        resolution_file.file_id = new_file_id;
        for export in &mut resolution_file.direct_exports {
            export.declaration = id_remap
                .get(&export.declaration)
                .copied()
                .unwrap_or(export.declaration);
        }
        for declaration in &mut resolution_file.top_level_declarations {
            declaration.declaration = id_remap
                .get(&declaration.declaration)
                .copied()
                .unwrap_or(declaration.declaration);
        }
        for method in &mut resolution_file.inherent_methods {
            method.declaration = id_remap
                .get(&method.declaration)
                .copied()
                .unwrap_or(method.declaration);
            method.owner = method
                .owner
                .map(|owner| id_remap.get(&owner).copied().unwrap_or(owner));
        }
        for rust_type in &mut resolution_file.rust_types {
            rust_type.declaration = id_remap
                .get(&rust_type.declaration)
                .copied()
                .unwrap_or(rust_type.declaration);
        }
        for rust_use in &mut resolution_file.rust_uses {
            rust_use.import = id_remap
                .get(&rust_use.import)
                .copied()
                .unwrap_or(rust_use.import);
        }
        for rust_module in &mut resolution_file.rust_modules {
            rust_module.declaration = rust_module
                .declaration
                .map(|declaration| id_remap.get(&declaration).copied().unwrap_or(declaration));
            for child in &mut rust_module.file_children {
                child.declaration = id_remap
                    .get(&child.declaration)
                    .copied()
                    .unwrap_or(child.declaration);
            }
        }
        for class in &mut resolution_file.classes {
            class.declaration = id_remap
                .get(&class.declaration)
                .copied()
                .unwrap_or(class.declaration);
            for method in &mut class.methods {
                method.declaration = id_remap
                    .get(&method.declaration)
                    .copied()
                    .unwrap_or(method.declaration);
            }
        }
        if let Some(c_cpp_file) = &mut resolution_file.c_cpp_file {
            c_cpp_file.source_path = full_path.to_path_buf();
            for namespace in &mut c_cpp_file.namespaces {
                namespace.declaration = id_remap
                    .get(&namespace.declaration)
                    .copied()
                    .unwrap_or(namespace.declaration);
            }
        }
    }

    if let Some(file_info) = artifact.files.first_mut() {
        file_info.id = new_file_id.0;
        file_info.path = full_path.to_path_buf();
        file_info.language = language_name.to_string();
        file_info.line_count = source.lines().count() as u32;
    }
    artifact.callable_projection_states =
        build_callable_projection_states(&artifact.nodes, &artifact.edges, &artifact.occurrences);
    artifact
}

fn incomplete_file_storage(
    path: &Path,
    source: Option<&str>,
    language: impl Into<String>,
    mut error: codestory_contracts::graph::ErrorInfo,
) -> IntermediateStorage {
    let source = source.unwrap_or("");
    let (file_node, _file_name, file_id) = file_node_from_source(path, source);
    let mut local_storage = IntermediateStorage::default();
    local_storage.files.push(codestory_store::FileInfo {
        id: file_id.0,
        path: path.to_path_buf(),
        language: language.into(),
        modification_time: file_modification_time(path),
        indexed: true,
        complete: false,
        line_count: source.lines().count() as u32,
        file_role: codestory_store::FileRole::classify_path(path),
    });
    local_storage.nodes.push(file_node);
    error.file_id = Some(file_id);
    local_storage.add_error(error);
    local_storage
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

/// Byte offsets of each line start, built once per file.
///
/// `source.lines().nth(n)` walks from byte zero on every call, and
/// `infer_access_from_source` calls it up to twice for every graph node — so
/// the cost was O(nodes x bytes) and measured at 66% of a 2 MB Rust index and
/// 36% of TypeScript (#1820). The offsets make each lookup O(line length).
///
/// The boundaries must match `str::lines()` exactly, because these lines feed
/// visibility classification and therefore the projected access of every
/// member. `lines()` splits on `\n`, strips one trailing `\r`, and does not
/// yield a final empty line for a source that ends in a newline.
struct LineOffsets {
    starts: Vec<usize>,
}

impl LineOffsets {
    fn new(source: &str) -> Self {
        let bytes = source.as_bytes();
        let mut starts =
            Vec::with_capacity(bytes.iter().filter(|byte| **byte == b'\n').count() + 1);
        starts.push(0);
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                starts.push(index + 1);
            }
        }
        // A trailing newline opens no line: `"a\n".lines()` yields just `"a"`.
        if starts.last() == Some(&bytes.len()) {
            starts.pop();
        }
        Self { starts }
    }

    /// The 1-based `line`, with its terminator stripped, or `None` past the end.
    fn line<'a>(&self, source: &'a str, line: u32) -> Option<&'a str> {
        let start = *self.starts.get(line.checked_sub(1)? as usize)?;
        let end = source[start..]
            .find('\n')
            .map_or(source.len(), |offset| start + offset);
        let text = &source[start..end];
        Some(text.strip_suffix('\r').unwrap_or(text))
    }
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
            let container_kind = node
                .parent()
                .map(|parent| parent.kind())
                .unwrap_or_default();
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

#[derive(Debug, Clone)]
struct ManualMemberEdgeSpec {
    source_name: String,
    target_name: String,
    source_span: GraphNodeSpan,
    target_span: GraphNodeSpan,
    line: Option<u32>,
}

#[derive(Debug, Clone)]
struct ManualReceiverCallSpec {
    source_name: String,
    source_span: GraphNodeSpan,
    receiver_name: String,
    owner_name: String,
    owner_module: Option<String>,
    method_name: String,
    method_col: Option<u32>,
    line: Option<u32>,
    allow_global_fallback: bool,
    /// Marker proving how the receiver was bound (PHP foreach element specs
    /// carry `receiver-binding:loop-element@{start}-{end}` with the exact
    /// foreach statement line range). Landed on the callsite edge through both
    /// engine branches, and appended even when an earlier spec already
    /// annotated or resolved the same callsite.
    binding_marker: Option<String>,
    /// Per-spec override for the callsite marker the annotate path requires.
    /// Construction specs require the language's `new` marker; member-call
    /// specs leave this `None` and keep the language default. A spec carrying
    /// an override is annotate-only: it never reaches the in-file
    /// owner+method lookup and never appends a fallback placeholder edge.
    required_callsite_marker: Option<&'static str>,
    /// The spec's source anchor is the enclosing CLASS node rather than a
    /// callable (P2: class anchoring is the written rule for constructor-body
    /// facts — C# emits no constructor node, so the enclosing class is the
    /// only stable anchor). Flagged specs resolve their source against CLASS
    /// nodes; unflagged specs keep the FUNCTION|METHOD lookup untouched.
    /// Flagged specs also never annotate the rule file's own placeholder:
    /// a constructor-body self-placeholder is never attributed to a callable
    /// and is dropped at post-processing, so owner markers must ride the
    /// spec's own placeholder edge instead.
    class_anchored: bool,
    /// The owner name was read off the call syntax itself (a
    /// `new X(args).Method()` chained call names X verbatim), not inferred
    /// from a binding. Syntactic owners are trustworthy enough to annotate the
    /// callsite with `receiver-owner:` even when no module is known, so the
    /// resolution pass's same-root-namespace arm can finish the job
    /// project-wide; inferred owners keep today's fail-closed behaviour.
    owner_is_syntactic: bool,
}

/// One type-usage fact a language collector proved against its own binding
/// tables (P2a).
///
/// A spec exists only when the collector resolved the type surface against
/// the file's visible/imported binding tables — that emit gate is what the
/// edge's `certainty = Some(Certain)` stamp asserts, because TYPE_USAGE has
/// no resolution job (resolution/pipeline.rs runs CALL/IMPORT/OVERRIDE only)
/// and emit-time is therefore the only place the certainty can come from.
#[derive(Debug, Clone)]
struct ManualTypeUsageSpec {
    source_name: String,
    source_span: GraphNodeSpan,
    /// Source anchor is the enclosing CLASS node rather than a callable
    /// (field declarations and constructor-context facts).
    class_anchored: bool,
    target_name: String,
    /// Fully qualified path of the target type when it resolved through an
    /// import table (`using` alias or single plain namespace import).
    target_module: Option<String>,
    /// Exact span of the same-file declaration node when the type is declared
    /// in this file; `None` for import-resolved types, whose edge lands on a
    /// reference node minted at the use site.
    target_declaration_span: Option<GraphNodeSpan>,
    /// Span of the type reference itself; places the reference node for
    /// import-resolved types.
    reference_span: GraphNodeSpan,
    line: Option<u32>,
    /// The referencing file's namespace, set when the type surface is a bare
    /// name none of the per-file tables could resolve. Such a spec emits a
    /// PENDING edge — certainty `None`, target a `type_ref_pending:` reference
    /// node — which `finalize_pending_type_usage_edges` either upgrades
    /// (exactly one project declaration with that name under the same root
    /// namespace) or deletes, after every file of the run has been flushed.
    /// `Some` here never coexists with a resolved target above.
    pending_namespace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReceiverCallSiteKey {
    receiver_name: String,
    method_name: String,
    line: Option<u32>,
    method_col: Option<u32>,
}

struct ManualReceiverSource<'a> {
    name: &'a str,
    span: GraphNodeSpan,
}

struct ReceiverPlaceholderAnnotation<'a> {
    line: Option<u32>,
    method_col: Option<u32>,
    method_name: &'a str,
    owner_name: &'a str,
    owner_module: Option<&'a str>,
    extra_callsite_marker: Option<&'a str>,
    binding_marker: Option<&'a str>,
}

struct CallPlaceholderMarkerAnnotation<'a> {
    line: Option<u32>,
    method_col: Option<u32>,
    method_name: &'a str,
    marker: &'static str,
}

/// Languages whose receiver-call placeholders must carry a syntax marker.
///
/// This is a narrower roster than `member_callsite_marker`: Kotlin owns a
/// marker in the registry and is deliberately absent here. There is no registry
/// field for it, so a migrated language keeps its arm and only repoints the
/// constant.
fn receiver_annotation_required_callsite_marker(language_name: &str) -> Option<&'static str> {
    match language_name {
        "python" => Some(languages::python::MEMBER_CALLSITE_MARKER),
        "php" => Some(languages::php::MEMBER_CALLSITE_MARKER),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct ImportedTypeBinding {
    module_name: String,
    owner_name: String,
}

type ReceiverOwnerBinding = (String, Option<String>);
type OptionalReceiverOwnerBinding = Option<ReceiverOwnerBinding>;

#[derive(Debug, Clone)]
struct ManualPreciseCallSpec {
    source_name: String,
    source_span: GraphNodeSpan,
    target_name: String,
    line: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct GraphNodeSpan {
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
}

fn graph_subspan_from_text_range(
    parent: GraphNodeSpan,
    text: &str,
    start_offset: usize,
    end_offset: usize,
) -> Option<GraphNodeSpan> {
    if start_offset >= end_offset
        || end_offset > text.len()
        || !text.is_char_boundary(start_offset)
        || !text.is_char_boundary(end_offset)
    {
        return None;
    }

    let prefix = &text[..start_offset];
    let matched = &text[start_offset..end_offset];
    let start_line = parent.start_line + prefix.bytes().filter(|b| *b == b'\n').count() as u32;
    let start_col = if let Some(last_newline) = prefix.rfind('\n') {
        prefix[last_newline + 1..].len() as u32 + 1
    } else {
        parent.start_col + prefix.len() as u32
    };
    let end_line = start_line + matched.bytes().filter(|b| *b == b'\n').count() as u32;
    let end_col = if let Some(last_newline) = matched.rfind('\n') {
        matched[last_newline + 1..].len() as u32 + 1
    } else {
        start_col + matched.len() as u32
    };

    Some(GraphNodeSpan {
        start_line,
        start_col,
        end_line,
        end_col,
    })
}

fn normalize_rust_impl_expr_surface(
    text: &str,
    span: GraphNodeSpan,
) -> Option<(String, GraphNodeSpan)> {
    let leading_ws = text.len().saturating_sub(text.trim_start().len());
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let base_end = trimmed.find('<').unwrap_or(trimmed.len());
    let base = trimmed[..base_end].trim_end();
    let segment_start = base.rfind("::").map(|idx| idx + 2).unwrap_or(0);
    let terminal = base[segment_start..].trim();
    if terminal.is_empty() {
        return None;
    }

    let start_offset = leading_ws + segment_start;
    let end_offset = start_offset + terminal.len();
    let normalized_span = graph_subspan_from_text_range(span, text, start_offset, end_offset)?;
    Some((terminal.to_string(), normalized_span))
}

fn rust_impl_expr_qualified_name(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let base_end = strip_trailing_generic_suffix_end(trimmed);
    let qualified = trimmed[..base_end].trim();
    (!qualified.is_empty()).then(|| qualified.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DeclarationSpanOverrideKey {
    kind: NodeKind,
    name: String,
    token_line: u32,
    token_col: u32,
}

fn trimmed_node_text(node: TsNode<'_>, source: &str) -> Option<String> {
    node_source_text(node, source)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn ts_node_graph_span(node: TsNode<'_>) -> GraphNodeSpan {
    let start = node.start_position();
    let end = node.end_position();
    GraphNodeSpan {
        start_line: start.row as u32 + 1,
        start_col: start.column as u32 + 1,
        end_line: end.row as u32 + 1,
        end_col: end.column as u32 + 1,
    }
}

fn insert_declaration_span_override(
    overrides: &mut HashMap<DeclarationSpanOverrideKey, GraphNodeSpan>,
    kind: NodeKind,
    name: String,
    token_node: TsNode<'_>,
    full_node: TsNode<'_>,
) {
    let token_span = ts_node_graph_span(token_node);
    overrides.insert(
        DeclarationSpanOverrideKey {
            kind,
            name,
            token_line: token_span.start_line,
            token_col: token_span.start_col,
        },
        ts_node_graph_span(full_node),
    );
}

fn first_named_child_with_kind<'tree>(node: TsNode<'tree>, kind: &str) -> Option<TsNode<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn c_like_declarator_name_node(node: TsNode<'_>) -> Option<TsNode<'_>> {
    match node.kind() {
        "identifier"
        | "field_identifier"
        | "type_identifier"
        | "qualified_identifier"
        | "namespace_identifier"
        | "destructor_name"
        | "operator_name" => Some(node),
        _ => node
            .child_by_field_name("declarator")
            .and_then(c_like_declarator_name_node)
            .or_else(|| {
                let mut cursor = node.walk();
                node.named_children(&mut cursor)
                    .find_map(c_like_declarator_name_node)
            }),
    }
}

fn c_specifier_span_node(node: TsNode<'_>) -> TsNode<'_> {
    node.parent()
        .filter(|parent| {
            parent.kind() == "declaration" && parent.child_by_field_name("declarator").is_none()
        })
        .unwrap_or(node)
}

fn java_named_child(node: TsNode<'_>) -> Option<TsNode<'_>> {
    node.child_by_field_name("name")
        .or_else(|| first_named_child_with_kind(node, "identifier"))
}

fn collect_c_declaration_span_overrides(
    tree: &Tree,
    source: &str,
    overrides: &mut HashMap<DeclarationSpanOverrideKey, GraphNodeSpan>,
) {
    walk_tree_nodes(tree.root_node(), &mut |node| match node.kind() {
        "struct_specifier" => {
            if let Some(name_node) = node.child_by_field_name("name")
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(overrides, NodeKind::CLASS, name, name_node, node);
            }
        }
        "union_specifier" => {
            if let Some(name_node) = node.child_by_field_name("name")
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(
                    overrides,
                    NodeKind::UNION,
                    name,
                    name_node,
                    c_specifier_span_node(node),
                );
            }
        }
        "enum_specifier" => {
            if let Some(name_node) = node.child_by_field_name("name")
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(
                    overrides,
                    NodeKind::ENUM,
                    name,
                    name_node,
                    c_specifier_span_node(node),
                );
            }
        }
        "enumerator" => {
            if let Some(name_node) = node.child_by_field_name("name")
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(
                    overrides,
                    NodeKind::ENUM_CONSTANT,
                    name,
                    name_node,
                    node,
                );
            }
        }
        "function_definition" | "declaration" => {
            if let Some(declarator) = node.child_by_field_name("declarator")
                && let Some(name_node) = c_like_declarator_name_node(declarator)
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(
                    overrides,
                    NodeKind::FUNCTION,
                    name,
                    name_node,
                    node,
                );
            }
        }
        _ => {}
    });
}

fn collect_cpp_declaration_span_overrides(
    tree: &Tree,
    source: &str,
    overrides: &mut HashMap<DeclarationSpanOverrideKey, GraphNodeSpan>,
) {
    walk_tree_nodes(tree.root_node(), &mut |node| match node.kind() {
        "class_specifier" | "struct_specifier" => {
            if let Some(name_node) = node.child_by_field_name("name")
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(overrides, NodeKind::CLASS, name, name_node, node);
            }
        }
        "enum_specifier" => {
            if let Some(name_node) = node.child_by_field_name("name")
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(overrides, NodeKind::ENUM, name, name_node, node);
            }
        }
        "enumerator" => {
            if let Some(name_node) = node.child_by_field_name("name")
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(
                    overrides,
                    NodeKind::ENUM_CONSTANT,
                    name,
                    name_node,
                    node,
                );
            }
        }
        "function_definition" => {
            if let Some(declarator) = node.child_by_field_name("declarator")
                && let Some(name_node) = c_like_declarator_name_node(declarator)
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(
                    overrides,
                    NodeKind::FUNCTION,
                    name,
                    name_node,
                    node,
                );
            }
        }
        _ => {}
    });
}

fn collect_java_declaration_span_overrides(
    tree: &Tree,
    source: &str,
    overrides: &mut HashMap<DeclarationSpanOverrideKey, GraphNodeSpan>,
) {
    walk_tree_nodes(tree.root_node(), &mut |node| match node.kind() {
        "class_declaration" => {
            if let Some(name_node) = java_named_child(node)
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(overrides, NodeKind::CLASS, name, name_node, node);
            }
        }
        "interface_declaration" => {
            if let Some(name_node) = java_named_child(node)
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(
                    overrides,
                    NodeKind::INTERFACE,
                    name,
                    name_node,
                    node,
                );
            }
        }
        "record_declaration" => {
            if let Some(name_node) = java_named_child(node)
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(overrides, NodeKind::CLASS, name, name_node, node);
            }
        }
        "enum_declaration" => {
            if let Some(name_node) = java_named_child(node)
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(overrides, NodeKind::ENUM, name, name_node, node);
            }
        }
        "annotation_type_declaration" => {
            if let Some(name_node) = java_named_child(node)
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(
                    overrides,
                    NodeKind::ANNOTATION,
                    name,
                    name_node,
                    node,
                );
            }
        }
        "method_declaration" | "constructor_declaration" | "compact_constructor_declaration" => {
            if let Some(name_node) = java_named_child(node)
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(
                    overrides,
                    NodeKind::METHOD,
                    name,
                    name_node,
                    node,
                );
            }
        }
        "field_declaration" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() != "variable_declarator" {
                    continue;
                }
                if let Some(name_node) = child.child_by_field_name("name")
                    && let Some(name) = trimmed_node_text(name_node, source)
                {
                    insert_declaration_span_override(
                        overrides,
                        NodeKind::FIELD,
                        name,
                        name_node,
                        node,
                    );
                }
            }
        }
        "enum_constant" => {
            if let Some(name_node) = java_named_child(node)
                && let Some(name) = trimmed_node_text(name_node, source)
            {
                insert_declaration_span_override(
                    overrides,
                    NodeKind::ENUM_CONSTANT,
                    name,
                    name_node,
                    node,
                );
            }
        }
        _ => {}
    });
}

fn collect_declaration_span_overrides(
    language_name: &str,
    tree: &Tree,
    source: &str,
) -> HashMap<DeclarationSpanOverrideKey, GraphNodeSpan> {
    let mut overrides = HashMap::new();
    match language_name {
        "c" => collect_c_declaration_span_overrides(tree, source, &mut overrides),
        "cpp" => collect_cpp_declaration_span_overrides(tree, source, &mut overrides),
        "java" => collect_java_declaration_span_overrides(tree, source, &mut overrides),
        _ => {}
    }
    overrides
}

#[derive(Debug, Clone)]
struct RuntimeImportSpec {
    binding_node_id: Option<NodeId>,
    module_node_id: NodeId,
    line: u32,
    suppress_line: u32,
    suppress_start_col: u32,
    suppress_callee_name: String,
    exact_bare_call_target_spans: Vec<GraphNodeSpan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeSpanPolicy {
    Definition,
    Token,
}

fn node_source_text(node: TsNode<'_>, source: &str) -> Option<String> {
    source.get(node.byte_range()).map(ToString::to_string)
}

fn graph_capture_span_policy(
    language_name: &str,
    kind: NodeKind,
    canonical_role: CanonicalNodeRole,
    rust_impl_expr: bool,
    name: &str,
    has_token_surface_edge: bool,
) -> NodeSpanPolicy {
    if rust_impl_expr || has_token_surface_edge {
        return NodeSpanPolicy::Token;
    }

    match (language_name, kind, canonical_role) {
        ("java", NodeKind::ANNOTATION, CanonicalNodeRole::Declaration) => {
            NodeSpanPolicy::Definition
        }
        ("java", NodeKind::ANNOTATION, _) => NodeSpanPolicy::Token,
        ("cpp", NodeKind::UNKNOWN, _) if name.contains("::") || name.contains('<') => {
            NodeSpanPolicy::Token
        }
        _ => NodeSpanPolicy::Definition,
    }
}

fn is_ascii_identifier_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_ascii_identifier_continue(byte: u8) -> bool {
    is_ascii_identifier_start(byte) || byte.is_ascii_digit()
}

fn trim_ascii_end_index(text: &str) -> usize {
    let mut end = text.len();
    let bytes = text.as_bytes();
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

fn strip_trailing_generic_suffix_end(text: &str) -> usize {
    let mut end = trim_ascii_end_index(text);
    let bytes = text.as_bytes();
    if end == 0 || bytes[end - 1] != b'>' {
        return end;
    }

    let mut depth = 0usize;
    let mut idx = end;
    while idx > 0 {
        idx -= 1;
        match bytes[idx] {
            b'>' => depth += 1,
            b'<' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                if depth == 0 {
                    end = idx;
                    break;
                }
            }
            _ => {}
        }
    }

    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

fn terminal_identifier_range(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut end = strip_trailing_generic_suffix_end(text);
    while end > 0 && !is_ascii_identifier_continue(bytes[end - 1]) {
        end -= 1;
    }
    if end == 0 {
        return None;
    }

    let mut start = end - 1;
    while start > 0 && is_ascii_identifier_continue(bytes[start - 1]) {
        start -= 1;
    }
    is_ascii_identifier_start(bytes[start]).then_some((start, end))
}

fn line_col_to_byte_offset(source: &str, line: u32, col: u32) -> Option<usize> {
    if line == 0 || col == 0 {
        return None;
    }

    let mut current_line = 1u32;
    let mut line_start = 0usize;
    if current_line < line {
        for (idx, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                current_line += 1;
                line_start = idx + 1;
                if current_line == line {
                    break;
                }
            }
        }
    }

    (current_line == line).then_some(line_start + col as usize - 1)
}

fn byte_offset_to_line_col(source: &str, offset: usize) -> Option<(u32, u32)> {
    if offset > source.len() {
        return None;
    }

    let mut line = 1u32;
    let mut line_start = 0usize;
    for (idx, byte) in source.bytes().enumerate() {
        if idx == offset {
            break;
        }
        if byte == b'\n' {
            line += 1;
            line_start = idx + 1;
        }
    }

    Some((line, offset.saturating_sub(line_start) as u32 + 1))
}

fn source_span_text(
    source: &str,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
) -> Option<(usize, &str)> {
    let start = line_col_to_byte_offset(source, start_line, start_col)?;
    let end = line_col_to_byte_offset(source, end_line, end_col)?;
    (start <= end && end <= source.len()).then_some((start, &source[start..end]))
}

fn extract_terminal_identifier_from_span(
    source: &str,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
) -> Option<(String, u32, u32, u32, u32)> {
    let (base_offset, text) = source_span_text(source, start_line, start_col, end_line, end_col)?;
    let (relative_start, relative_end) = terminal_identifier_range(text)?;
    let absolute_start = base_offset + relative_start;
    let absolute_end = base_offset + relative_end;
    let (token_start_line, token_start_col) = byte_offset_to_line_col(source, absolute_start)?;
    let (token_end_line, token_end_col) = byte_offset_to_line_col(source, absolute_end)?;
    Some((
        text[relative_start..relative_end].to_string(),
        token_start_line,
        token_start_col,
        token_end_line,
        token_end_col,
    ))
}

struct GraphCaptureNormalizationInput<'a> {
    language_name: &'a str,
    kind: NodeKind,
    canonical_role: CanonicalNodeRole,
    rust_impl_expr: bool,
    name: &'a str,
    graph_span: GraphNodeSpan,
    source: &'a str,
    has_token_surface_edge: bool,
}

fn normalize_graph_capture(
    input: &GraphCaptureNormalizationInput<'_>,
) -> Option<(String, u32, u32, u32, u32)> {
    if input.language_name == "rust" && input.rust_impl_expr {
        let (normalized_name, normalized_span) =
            normalize_rust_impl_expr_surface(input.name, input.graph_span)?;
        return Some((
            normalized_name,
            normalized_span.start_line,
            normalized_span.start_col,
            normalized_span.end_line,
            normalized_span.end_col,
        ));
    }

    if input.language_name == "rust" && input.canonical_role == CanonicalNodeRole::ImplAnchor {
        return extract_terminal_identifier_from_span(
            input.source,
            input.graph_span.start_line,
            input.graph_span.start_col,
            input.graph_span.end_line,
            input.graph_span.end_col,
        );
    }

    if cpp_unknown_capture_needs_terminal_identifier(input) {
        return extract_terminal_identifier_from_span(
            input.source,
            input.graph_span.start_line,
            input.graph_span.start_col,
            input.graph_span.end_line,
            input.graph_span.end_col,
        );
    }

    None
}

fn cpp_unknown_capture_needs_terminal_identifier(
    input: &GraphCaptureNormalizationInput<'_>,
) -> bool {
    let is_cpp_unknown_capture = input.language_name == "cpp" && input.kind == NodeKind::UNKNOWN;
    let has_composite_surface =
        cpp_name_is_scoped_or_template(input.name) || input.has_token_surface_edge;
    is_cpp_unknown_capture && has_composite_surface
}

fn cpp_name_is_scoped_or_template(name: &str) -> bool {
    name.contains("::") || name.contains('<')
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

fn walk_tree_nodes<'tree, F>(node: TsNode<'tree>, visit: &mut F)
where
    F: FnMut(TsNode<'tree>),
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

#[derive(Debug, Clone)]
struct RustReceiverCallHint {
    method_name: String,
    qualified_method_name: String,
    start_line: u32,
    start_col: u32,
}

type RustStructFieldTypes = HashMap<(String, String), String>;
type RustMethodReturnTypes = HashMap<(String, String), String>;
type RustTypeAliases = HashMap<String, String>;
type RustTraitMethods = HashMap<String, HashSet<String>>;

fn collect_rust_receiver_call_hints(tree: &Tree, source: &str) -> Vec<RustReceiverCallHint> {
    let aliases = collect_rust_type_aliases(tree, source);
    let field_types = collect_rust_struct_field_types(tree, source, &aliases);
    let method_return_types = collect_rust_method_return_types(tree, source, &aliases);
    let local_trait_methods = collect_rust_trait_methods(tree, source, &aliases);
    let local_unit_structs = collect_rust_unit_structs(tree, source, &aliases);
    let mut hints = Vec::new();
    let mut scopes: HashMap<usize, RustValueScope<'_>> = HashMap::new();

    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "call_expression" {
            return;
        }
        let Some(function_node) = node.child_by_field_name("function") else {
            return;
        };
        if function_node.kind() != "field_expression" {
            return;
        }
        let Some(method_node) = function_node.child_by_field_name("field") else {
            return;
        };
        let Some(method_name) = node_source_text(method_node, source)
            .map(|value| value.trim().to_string())
            .filter(|value| is_rust_identifier_like(value))
        else {
            return;
        };
        let Some(receiver_node) = function_node.child_by_field_name("value") else {
            return;
        };
        let impl_owner = rust_enclosing_impl_owner(node, source, &aliases);
        let self_owner = rust_enclosing_self_owner(node, source, &aliases);
        // One scope per enclosing function, advanced to this call rather than
        // rebuilt for it. `no_scope` covers a call outside any function, which
        // previously produced an empty map by finding no `function_item`.
        let empty_value_types = HashMap::new();
        let value_types = match rust_enclosing_function_item(node) {
            Some(function_node) => {
                let scope = scopes
                    .entry(function_node.id())
                    .or_insert_with(|| RustValueScope::new(function_node, impl_owner.clone()));
                scope.advance_to(
                    node.start_byte(),
                    source,
                    &aliases,
                    &field_types,
                    &method_return_types,
                    &local_unit_structs,
                );
                &scope.value_types
            }
            None => &empty_value_types,
        };
        let direct_self_owner = match receiver_node.kind() {
            "self" => self_owner.clone(),
            "identifier" if node_source_text(receiver_node, source).as_deref() == Some("Self") => {
                self_owner.clone()
            }
            _ => None,
        };
        let Some(mut owner_name) = direct_self_owner
            .or_else(|| {
                infer_rust_receiver_owner(
                    receiver_node,
                    source,
                    impl_owner.as_deref(),
                    &field_types,
                    &method_return_types,
                    value_types,
                    &aliases,
                )
            })
            .filter(|value| is_rust_type_like_name(value))
        else {
            return;
        };
        if rust_enclosing_generic_type_params(node, source).contains(&owner_name) {
            let Some(bound_owner) = rust_local_generic_bound_owner_for_method(
                node,
                &owner_name,
                &method_name,
                source,
                &aliases,
                &local_trait_methods,
            ) else {
                return;
            };
            owner_name = bound_owner;
        }
        let position = method_node.start_position();
        hints.push(RustReceiverCallHint {
            method_name: method_name.clone(),
            qualified_method_name: format!("{owner_name}::{method_name}"),
            start_line: position.row as u32 + 1,
            start_col: position.column as u32 + 1,
        });
    });

    hints
}

fn collect_rust_unit_structs(
    tree: &Tree,
    source: &str,
    aliases: &RustTypeAliases,
) -> HashSet<String> {
    let mut owners = HashSet::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "struct_item" || node.child_by_field_name("body").is_some() {
            return;
        }
        if let Some(owner) = node
            .child_by_field_name("name")
            .and_then(|name| node_source_text(name, source))
            .and_then(|name| normalize_rust_type_owner_name(&name, aliases))
        {
            owners.insert(owner);
        }
    });
    owners
}

fn collect_rust_trait_methods(
    tree: &Tree,
    source: &str,
    aliases: &RustTypeAliases,
) -> RustTraitMethods {
    let mut traits = RustTraitMethods::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "trait_item" {
            return;
        }
        let Some(owner) = node
            .child_by_field_name("name")
            .and_then(|name| node_source_text(name, source))
            .and_then(|name| normalize_rust_type_owner_name(&name, aliases))
        else {
            return;
        };
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let mut methods = HashSet::new();
        let mut cursor = body.walk();
        for item in body.named_children(&mut cursor) {
            if !matches!(item.kind(), "function_signature_item" | "function_item") {
                continue;
            }
            if let Some(method) = item
                .child_by_field_name("name")
                .and_then(|name| node_source_text(name, source))
                .map(|name| name.trim().to_string())
                .filter(|name| is_rust_identifier_like(name))
            {
                methods.insert(method);
            }
        }
        traits.insert(owner, methods);
    });
    traits
}

fn rust_local_generic_bound_owner_for_method(
    node: TsNode<'_>,
    generic_name: &str,
    method_name: &str,
    source: &str,
    aliases: &RustTypeAliases,
    local_trait_methods: &RustTraitMethods,
) -> Option<String> {
    let mut cursor = Some(node);
    while let Some(current) = cursor {
        if matches!(
            current.kind(),
            "function_item" | "impl_item" | "struct_item" | "enum_item" | "trait_item"
        ) {
            let (declares_generic, owners) =
                rust_generic_bound_owners(current, generic_name, source, aliases);
            if declares_generic || !owners.is_empty() {
                let mut candidates = owners
                    .into_iter()
                    .filter(|owner| {
                        local_trait_methods
                            .get(owner)
                            .is_some_and(|methods| methods.contains(method_name))
                    })
                    .collect::<Vec<_>>();
                candidates.sort();
                candidates.dedup();
                return match candidates.as_slice() {
                    [owner] => Some(owner.clone()),
                    _ => None,
                };
            }
        }
        cursor = current.parent();
    }
    None
}

fn rust_generic_bound_owners(
    boundary: TsNode<'_>,
    generic_name: &str,
    source: &str,
    aliases: &RustTypeAliases,
) -> (bool, HashSet<String>) {
    let mut declares_generic = false;
    let mut owners = HashSet::new();
    if let Some(parameters) = boundary.child_by_field_name("type_parameters") {
        let mut cursor = parameters.walk();
        for parameter in parameters.named_children(&mut cursor) {
            if parameter.kind() != "type_parameter"
                || parameter
                    .child_by_field_name("name")
                    .and_then(|name| node_source_text(name, source))
                    .as_deref()
                    .map(str::trim)
                    != Some(generic_name)
            {
                continue;
            }
            declares_generic = true;
            if let Some(bounds) = parameter.child_by_field_name("bounds") {
                collect_rust_bound_owners(bounds, source, aliases, &mut owners);
            }
        }
    }

    let mut boundary_cursor = boundary.walk();
    for child in boundary.named_children(&mut boundary_cursor) {
        if child.kind() != "where_clause" {
            continue;
        }
        let mut where_cursor = child.walk();
        for predicate in child.named_children(&mut where_cursor) {
            if predicate.kind() != "where_predicate"
                || predicate
                    .child_by_field_name("left")
                    .and_then(|left| node_source_text(left, source))
                    .as_deref()
                    .map(str::trim)
                    != Some(generic_name)
            {
                continue;
            }
            if let Some(bounds) = predicate.child_by_field_name("bounds") {
                collect_rust_bound_owners(bounds, source, aliases, &mut owners);
            }
        }
    }
    (declares_generic, owners)
}

fn collect_rust_bound_owners(
    bounds: TsNode<'_>,
    source: &str,
    aliases: &RustTypeAliases,
    owners: &mut HashSet<String>,
) {
    let mut cursor = bounds.walk();
    for bound in bounds.named_children(&mut cursor) {
        if bound.kind() == "lifetime" {
            continue;
        }
        if let Some(owner) = node_source_text(bound, source)
            .and_then(|surface| normalize_rust_type_owner_name(&surface, aliases))
        {
            owners.insert(owner);
        }
    }
}

fn apply_rust_receiver_call_hints(
    tree: &Tree,
    source: &str,
    unique_nodes: &mut HashMap<NodeId, Node>,
) {
    let hints = collect_rust_receiver_call_hints(tree, source);
    if hints.is_empty() {
        return;
    }

    // Both operands grow with file size, so the original nested scan was
    // O(hints x nodes) and measured at 20% of a 2 MB Rust index (#1820).
    // Indexing the unresolved nodes by call site makes it O(hints + nodes).
    let mut unresolved_by_site: HashMap<(&str, u32, u32), Vec<NodeId>> = HashMap::new();
    for (id, node) in unique_nodes.iter() {
        if node.kind != NodeKind::UNKNOWN {
            continue;
        }
        let (Some(start_line), Some(start_col)) = (node.start_line, node.start_col) else {
            continue;
        };
        unresolved_by_site
            .entry((node.serialized_name.as_str(), start_line, start_col))
            .or_default()
            .push(*id);
    }
    let matched = hints
        .iter()
        .filter_map(|hint| {
            let key = (hint.method_name.as_str(), hint.start_line, hint.start_col);
            // Taken, not read: once a site is rewritten its nodes carry the
            // qualified name and can no longer match this key, which is what
            // the sequential scan did by re-reading `serialized_name`.
            unresolved_by_site.remove(&key).map(|ids| (hint, ids))
        })
        .map(|(hint, ids)| (hint.qualified_method_name.clone(), ids))
        .collect::<Vec<_>>();
    for (qualified_method_name, ids) in matched {
        for id in ids {
            if let Some(node) = unique_nodes.get_mut(&id) {
                node.serialized_name = qualified_method_name.clone();
                node.qualified_name = Some(qualified_method_name.clone());
            }
        }
    }
}

fn collect_rust_type_aliases(tree: &Tree, source: &str) -> RustTypeAliases {
    let mut aliases = RustTypeAliases::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "use_declaration" {
            return;
        }
        let alias_surface = node
            .child_by_field_name("argument")
            .and_then(|argument| node_source_text(argument, source))
            .or_else(|| node_source_text(node, source));
        let Some(alias_surface) = alias_surface else {
            return;
        };
        collect_rust_aliases_from_surface(&alias_surface, &mut aliases);
    });
    aliases
}

fn collect_rust_aliases_from_surface(surface: &str, aliases: &mut RustTypeAliases) {
    for segment in surface.split(',') {
        let segment = segment
            .trim()
            .trim_start_matches("use ")
            .trim_end_matches(';')
            .trim();
        let Some((raw_target, raw_alias)) = segment.rsplit_once(" as ") else {
            continue;
        };
        let Some(alias) = rust_tail_identifier(raw_alias) else {
            continue;
        };
        let Some(target) = rust_tail_identifier(raw_target) else {
            continue;
        };
        if is_rust_type_like_name(&alias) && is_rust_type_like_name(&target) {
            aliases.insert(alias, target);
        }
    }
}

fn collect_rust_struct_field_types(
    tree: &Tree,
    source: &str,
    aliases: &RustTypeAliases,
) -> RustStructFieldTypes {
    let mut fields = RustStructFieldTypes::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "struct_item" {
            return;
        }
        let Some(owner) = node
            .child_by_field_name("name")
            .and_then(|name| node_source_text(name, source))
            .and_then(|name| normalize_rust_type_owner_name(&name, aliases))
        else {
            return;
        };
        walk_tree_nodes(node, &mut |field_node| {
            if field_node.kind() != "field_declaration" {
                return;
            }
            let Some(field_name) = field_node
                .child_by_field_name("name")
                .and_then(|name| node_source_text(name, source))
                .map(|name| name.trim().to_string())
                .filter(|name| is_rust_identifier_like(name))
            else {
                return;
            };
            let Some(field_type) = field_node
                .child_by_field_name("type")
                .and_then(|ty| node_source_text(ty, source))
                .and_then(|ty| normalize_rust_type_owner_name(&ty, aliases))
            else {
                return;
            };
            fields.insert((owner.clone(), field_name), field_type);
        });
    });
    fields
}

fn collect_rust_method_return_types(
    tree: &Tree,
    source: &str,
    aliases: &RustTypeAliases,
) -> RustMethodReturnTypes {
    let mut return_types = RustMethodReturnTypes::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "function_item" {
            return;
        }
        let Some(owner) = rust_enclosing_impl_owner(node, source, aliases) else {
            return;
        };
        let Some(method_name) = node
            .child_by_field_name("name")
            .and_then(|name| node_source_text(name, source))
            .map(|name| name.trim().to_string())
            .filter(|name| is_rust_identifier_like(name))
        else {
            return;
        };
        let Some(return_type) = rust_function_return_type_text(node, source)
            .and_then(|ty| normalize_rust_return_owner_name(&ty, aliases, Some(&owner)))
        else {
            return;
        };
        return_types.insert((owner, method_name), return_type);
    });
    return_types
}

fn rust_function_return_type_text(function_node: TsNode<'_>, source: &str) -> Option<String> {
    if let Some(return_node) = function_node
        .child_by_field_name("return_type")
        .or_else(|| {
            let mut cursor = function_node.walk();
            function_node
                .named_children(&mut cursor)
                .find(|child| child.kind() == "return_type")
        })
        && let Some(return_type) = node_source_text(return_node, source)
    {
        return Some(
            return_type
                .trim()
                .trim_start_matches("->")
                .trim()
                .to_string(),
        );
    }

    let surface = node_source_text(function_node, source)?;
    let signature = surface.split('{').next().unwrap_or(&surface);
    let (_, return_type) = signature.rsplit_once("->")?;
    Some(return_type.trim().to_string())
}

fn rust_enclosing_impl_owner(
    node: TsNode<'_>,
    source: &str,
    aliases: &RustTypeAliases,
) -> Option<String> {
    let mut cursor = Some(node);
    while let Some(current) = cursor {
        if current.kind() == "impl_item" {
            return current
                .child_by_field_name("type")
                .and_then(|ty| node_source_text(ty, source))
                .and_then(|ty| normalize_rust_type_owner_name(&ty, aliases));
        }
        cursor = current.parent();
    }
    None
}

fn rust_enclosing_self_owner(
    node: TsNode<'_>,
    source: &str,
    aliases: &RustTypeAliases,
) -> Option<String> {
    let mut cursor = Some(node);
    while let Some(current) = cursor {
        let owner = match current.kind() {
            "impl_item" => current.child_by_field_name("type"),
            "trait_item" => current.child_by_field_name("name"),
            _ => None,
        };
        if let Some(owner) = owner
            .and_then(|owner| node_source_text(owner, source))
            .and_then(|owner| normalize_rust_type_owner_name(&owner, aliases))
        {
            return Some(owner);
        }
        cursor = current.parent();
    }
    None
}

/// One function's value bindings, resolved as call sites are reached.
///
/// The old shape rebuilt this map for *every* call by walking the enclosing
/// function's whole subtree, so a function with K calls and N nodes cost
/// O(K x N). That is the entire 110x blow-up in #1820: at a fixed ~500 KB,
/// moving statements from many small functions into one giant one took
/// `index_file` from ~1.2 s to ~134 s.
///
/// Two properties make a single pass equivalent rather than merely similar:
///
/// * a binding's inferred type depends only on the bindings *before* it, and
/// * `walk_tree_nodes` is pre-order, so it visits nodes in non-decreasing
///   `start_byte` — which makes the old `start_byte <= call_start_byte` filter
///   a prefix of the walk, not an arbitrary subset of it.
///
/// So the insertion sequence is the same for every call in the function, and
/// each call needs a prefix of it. Calls arrive in non-decreasing byte order
/// too, so the cursor only ever moves forward and the whole function costs
/// O(N) instead of O(K x N).
struct RustValueScope<'tree> {
    bindings: Vec<TsNode<'tree>>,
    cursor: usize,
    value_types: HashMap<String, String>,
    impl_owner: Option<String>,
}

impl<'tree> RustValueScope<'tree> {
    fn new(function_node: TsNode<'tree>, impl_owner: Option<String>) -> Self {
        let mut bindings = Vec::new();
        walk_tree_nodes(function_node, &mut |node| {
            if matches!(node.kind(), "parameter" | "let_declaration") {
                bindings.push(node);
            }
        });
        Self {
            bindings,
            cursor: 0,
            value_types: HashMap::new(),
            impl_owner,
        }
    }

    /// Fold in every binding that starts at or before `call_start_byte`.
    fn advance_to(
        &mut self,
        call_start_byte: usize,
        source: &str,
        aliases: &RustTypeAliases,
        field_types: &RustStructFieldTypes,
        method_return_types: &RustMethodReturnTypes,
        local_unit_structs: &HashSet<String>,
    ) {
        while let Some(binding) = self.bindings.get(self.cursor) {
            if binding.start_byte() > call_start_byte {
                break;
            }
            self.cursor += 1;
            let binding = *binding;
            let Some(pattern_node) = binding.child_by_field_name("pattern") else {
                continue;
            };
            let Some(value_name) = rust_pattern_identifier(pattern_node, source) else {
                continue;
            };
            let type_name = binding
                .child_by_field_name("type")
                .and_then(|ty| node_source_text(ty, source))
                .and_then(|ty| normalize_rust_type_owner_name(&ty, aliases))
                .or_else(|| {
                    if binding.kind() != "let_declaration" {
                        return None;
                    }
                    binding.child_by_field_name("value").and_then(|value| {
                        infer_rust_value_owner_from_expression(
                            value,
                            source,
                            self.impl_owner.as_deref(),
                            field_types,
                            method_return_types,
                            &self.value_types,
                            aliases,
                        )
                        .or_else(|| {
                            rust_direct_local_unit_struct_owner(
                                value,
                                source,
                                aliases,
                                local_unit_structs,
                            )
                        })
                    })
                });
            let Some(type_name) = type_name else {
                continue;
            };
            self.value_types.insert(value_name, type_name);
        }
    }
}

fn rust_direct_local_unit_struct_owner(
    value: TsNode<'_>,
    source: &str,
    aliases: &RustTypeAliases,
    local_unit_structs: &HashSet<String>,
) -> Option<String> {
    if !matches!(value.kind(), "identifier" | "scoped_identifier") {
        return None;
    }
    let owner = node_source_text(value, source)
        .and_then(|surface| normalize_rust_type_owner_name(&surface, aliases))?;
    local_unit_structs.contains(&owner).then_some(owner)
}

fn rust_enclosing_function_item(call_node: TsNode<'_>) -> Option<TsNode<'_>> {
    let mut cursor = call_node.parent();
    while let Some(current) = cursor {
        if current.kind() == "function_item" {
            return Some(current);
        }
        cursor = current.parent();
    }
    None
}

fn infer_rust_receiver_owner(
    receiver_node: TsNode<'_>,
    source: &str,
    impl_owner: Option<&str>,
    field_types: &RustStructFieldTypes,
    method_return_types: &RustMethodReturnTypes,
    value_types: &HashMap<String, String>,
    aliases: &RustTypeAliases,
) -> Option<String> {
    match receiver_node.kind() {
        "self" => impl_owner.map(str::to_string),
        "identifier" => node_source_text(receiver_node, source)
            .map(|name| name.trim().to_string())
            .and_then(|name| {
                if name == "Self" {
                    impl_owner.map(str::to_string)
                } else {
                    value_types.get(&name).cloned()
                }
            }),
        "field_expression" => {
            let field_name = receiver_node
                .child_by_field_name("field")
                .and_then(|field| node_source_text(field, source))
                .map(|name| name.trim().to_string())
                .filter(|name| is_rust_identifier_like(name))?;
            let value_node = receiver_node.child_by_field_name("value")?;
            let owner_name = infer_rust_receiver_owner(
                value_node,
                source,
                impl_owner,
                field_types,
                method_return_types,
                value_types,
                aliases,
            )?;
            field_types.get(&(owner_name, field_name)).cloned()
        }
        "call_expression" | "try_expression" | "parenthesized_expression" => {
            infer_rust_value_owner_from_expression(
                receiver_node,
                source,
                impl_owner,
                field_types,
                method_return_types,
                value_types,
                aliases,
            )
        }
        _ => None,
    }
}

fn infer_rust_value_owner_from_expression(
    expr_node: TsNode<'_>,
    source: &str,
    impl_owner: Option<&str>,
    field_types: &RustStructFieldTypes,
    method_return_types: &RustMethodReturnTypes,
    value_types: &HashMap<String, String>,
    aliases: &RustTypeAliases,
) -> Option<String> {
    match expr_node.kind() {
        "call_expression" => infer_rust_call_return_owner(
            expr_node,
            source,
            impl_owner,
            field_types,
            method_return_types,
            value_types,
            aliases,
        ),
        "try_expression" | "parenthesized_expression" | "await_expression" => {
            let mut cursor = expr_node.walk();
            expr_node.named_children(&mut cursor).find_map(|child| {
                infer_rust_value_owner_from_expression(
                    child,
                    source,
                    impl_owner,
                    field_types,
                    method_return_types,
                    value_types,
                    aliases,
                )
            })
        }
        "if_expression" => {
            let mut inferred = HashSet::new();
            walk_tree_nodes(expr_node, &mut |node| {
                if node.kind() != "call_expression" {
                    return;
                }
                if let Some(owner) = infer_rust_call_return_owner(
                    node,
                    source,
                    impl_owner,
                    field_types,
                    method_return_types,
                    value_types,
                    aliases,
                ) {
                    inferred.insert(owner);
                }
            });
            (inferred.len() == 1)
                .then(|| inferred.into_iter().next())
                .flatten()
        }
        "field_expression" | "identifier" | "self" => infer_rust_receiver_owner(
            expr_node,
            source,
            impl_owner,
            field_types,
            method_return_types,
            value_types,
            aliases,
        ),
        _ => None,
    }
}

fn infer_rust_call_return_owner(
    call_node: TsNode<'_>,
    source: &str,
    impl_owner: Option<&str>,
    field_types: &RustStructFieldTypes,
    method_return_types: &RustMethodReturnTypes,
    value_types: &HashMap<String, String>,
    aliases: &RustTypeAliases,
) -> Option<String> {
    let function_node = call_node.child_by_field_name("function")?;
    match function_node.kind() {
        "field_expression" => {
            let method_name = function_node
                .child_by_field_name("field")
                .and_then(|field| node_source_text(field, source))
                .map(|name| name.trim().to_string())
                .filter(|name| is_rust_identifier_like(name))?;
            let value_node = function_node.child_by_field_name("value")?;
            let receiver_owner = infer_rust_receiver_owner(
                value_node,
                source,
                impl_owner,
                field_types,
                method_return_types,
                value_types,
                aliases,
            )?;
            method_return_types
                .get(&(receiver_owner.clone(), method_name.clone()))
                .cloned()
                .or_else(|| {
                    rust_type_preserving_adapter_method(&method_name).then_some(receiver_owner)
                })
        }
        "scoped_identifier" => {
            let (owner_name, method_name) =
                rust_scoped_function_owner_and_name(function_node, source, aliases, impl_owner)?;
            method_return_types
                .get(&(owner_name.clone(), method_name.clone()))
                .cloned()
                .or_else(|| {
                    rust_constructor_like_associated_method(&method_name).then_some(owner_name)
                })
        }
        _ => None,
    }
}

fn rust_scoped_function_owner_and_name(
    function_node: TsNode<'_>,
    source: &str,
    aliases: &RustTypeAliases,
    impl_owner: Option<&str>,
) -> Option<(String, String)> {
    let surface = node_source_text(function_node, source)?;
    let (raw_owner, raw_method) = surface.rsplit_once("::")?;
    let mut owner_name = normalize_rust_type_owner_name(raw_owner, aliases)?;
    if owner_name == "Self" {
        owner_name = impl_owner?.to_string();
    }
    let method_name = rust_tail_identifier(raw_method)?;
    Some((owner_name, method_name))
}

fn rust_type_preserving_adapter_method(method_name: &str) -> bool {
    matches!(
        method_name,
        "map_err" | "context" | "with_context" | "inspect" | "inspect_err"
    )
}

fn rust_constructor_like_associated_method(method_name: &str) -> bool {
    method_name == "new"
        || method_name == "open"
        || method_name == "default"
        || method_name == "from"
        || method_name.starts_with("new_")
}

fn rust_pattern_identifier(pattern_node: TsNode<'_>, source: &str) -> Option<String> {
    if pattern_node.kind() == "identifier" {
        return node_source_text(pattern_node, source)
            .map(|name| name.trim().to_string())
            .filter(|name| is_rust_identifier_like(name));
    }

    let mut found = None;
    walk_tree_nodes(pattern_node, &mut |node| {
        if found.is_none() && node.kind() == "identifier" {
            found = node_source_text(node, source)
                .map(|name| name.trim().to_string())
                .filter(|name| is_rust_identifier_like(name));
        }
    });
    found
}

fn rust_enclosing_generic_type_params(node: TsNode<'_>, source: &str) -> HashSet<String> {
    let mut params = HashSet::new();
    let mut cursor = Some(node);
    while let Some(current) = cursor {
        if matches!(
            current.kind(),
            "function_item" | "impl_item" | "struct_item" | "enum_item" | "trait_item"
        ) {
            collect_rust_generic_type_params(current, source, &mut params);
        }
        cursor = current.parent();
    }
    params
}

fn collect_rust_generic_type_params(node: TsNode<'_>, source: &str, params: &mut HashSet<String>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "type_parameters" {
            continue;
        }
        let Some(raw_params) = node_source_text(child, source) else {
            continue;
        };
        for part in split_top_level_type_arguments(&raw_params) {
            if let Some(name) = rust_generic_param_name(&part) {
                params.insert(name);
            }
        }
    }
}

fn rust_generic_param_name(value: &str) -> Option<String> {
    let raw = value.trim();
    if raw.starts_with('\'') || raw.is_empty() {
        return None;
    }
    let name = raw
        .split(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric()))
        .next()
        .unwrap_or_default();
    is_rust_identifier_like(name).then(|| name.to_string())
}

fn normalize_rust_return_owner_name(
    raw_type: &str,
    aliases: &RustTypeAliases,
    impl_owner: Option<&str>,
) -> Option<String> {
    let mut value = raw_type
        .trim()
        .trim_start_matches("->")
        .trim()
        .trim_end_matches(';')
        .trim();
    if let Some((before_where, _)) = value.split_once(" where ") {
        value = before_where.trim();
    }
    if let Some(rest) = value.strip_prefix("impl ") {
        value = rest.trim();
    }

    let owner_surface = value
        .find('<')
        .and_then(|generic_start| {
            let owner = rust_tail_identifier(&value[..generic_start])?;
            matches!(
                owner.as_str(),
                "Result" | "Option" | "Box" | "Arc" | "Rc" | "Cow"
            )
            .then(|| split_top_level_type_arguments(value).into_iter().next())
            .flatten()
        })
        .unwrap_or_else(|| value.to_string());

    let mut owner = normalize_rust_type_owner_name(&owner_surface, aliases)?;
    if owner == "Self" {
        owner = impl_owner?.to_string();
    }
    Some(owner)
}

fn normalize_rust_type_owner_name(raw_type: &str, aliases: &RustTypeAliases) -> Option<String> {
    let mut value = raw_type.trim();
    loop {
        let trimmed = value.trim_start();
        if let Some(rest) = trimmed.strip_prefix('&') {
            value = rest.trim_start();
            if let Some(rest) = strip_rust_lifetime_prefix(value) {
                value = rest;
            }
            if let Some(rest) = value.strip_prefix("mut ") {
                value = rest.trim_start();
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("mut ") {
            value = rest.trim_start();
            continue;
        }
        value = trimmed;
        break;
    }

    if let Some(rest) = value.strip_prefix("dyn ") {
        value = rest.trim_start();
    }
    if value == "Self" {
        return Some(value.to_string());
    }

    let generic_start = value.find('<').unwrap_or(value.len());
    let owner_surface = value[..generic_start].trim();
    let mut owner = rust_tail_identifier(owner_surface)?;
    if let Some(alias_target) = aliases.get(&owner) {
        owner = alias_target.clone();
    }
    is_rust_type_like_name(&owner).then_some(owner)
}

fn strip_rust_lifetime_prefix(value: &str) -> Option<&str> {
    let value = value.trim_start();
    let rest = value.strip_prefix('\'')?;
    let end = rest
        .char_indices()
        .find_map(|(idx, ch)| (!(ch == '_' || ch.is_ascii_alphanumeric())).then_some(idx))
        .unwrap_or(rest.len());
    Some(rest[end..].trim_start())
}

fn rust_tail_identifier(surface: &str) -> Option<String> {
    let trimmed = surface
        .trim()
        .trim_matches(|ch: char| matches!(ch, '{' | '}' | '(' | ')' | ';'));
    let tail = trimmed
        .rsplit(|ch: char| !(ch == '_' || ch == ':' || ch.is_ascii_alphanumeric()))
        .find(|part| !part.is_empty())
        .unwrap_or(trimmed)
        .rsplit("::")
        .next()
        .unwrap_or(trimmed)
        .trim();
    is_rust_identifier_like(tail).then(|| tail.to_string())
}

fn is_rust_identifier_like(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_rust_type_like_name(value: &str) -> bool {
    value
        .chars()
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_uppercase())
}

fn collect_cpp_template_type_argument_edges(tree: &Tree, source: &str) -> Vec<ManualEdgeSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "template_type" {
            return;
        }
        let Some(template_name) = cpp_named_type_text(node.child_by_field_name("name"), source)
        else {
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
        "type_identifier"
        | "qualified_identifier"
        | "primitive_type"
        | "identifier"
        | "namespace_identifier"
        | "field_identifier" => node_source_text(node, source).map(|text| {
            text.trim()
                .trim_start_matches("typename ")
                .trim_start_matches("class ")
                .trim()
                .to_string()
        }),
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

fn tsx_owner_name(mut node: TsNode<'_>, source: &str) -> Option<String> {
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "function_declaration" | "method_definition" => {
                return parent
                    .child_by_field_name("name")
                    .and_then(|name| node_source_text(name, source))
                    .map(|name| name.trim().to_string());
            }
            "arrow_function" | "function_expression" => {
                return parent
                    .parent()
                    .and_then(|owner| tsx_callable_binding_name(owner, source));
            }
            _ => {
                node = parent;
            }
        }
    }
    None
}

fn tsx_callable_binding_name(node: TsNode<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "variable_declarator" | "public_field_definition" | "property_signature" => node
            .child_by_field_name("name")
            .and_then(|name| node_source_text(name, source))
            .map(|name| name.trim().to_string()),
        "pair" | "property_assignment" => node
            .child_by_field_name("key")
            .and_then(|name| node_source_text(name, source))
            .map(|name| name.trim().to_string()),
        _ => None,
    }
}

fn is_probable_jsx_component_name(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.contains('.') {
        return true;
    }
    trimmed
        .chars()
        .next()
        .map(|ch| ch.is_ascii_uppercase())
        .unwrap_or(false)
}

fn jsx_element_name(node: TsNode<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("name")
        .and_then(|name| node_source_text(name, source))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

fn jsx_attribute_target_name(node: TsNode<'_>, source: &str) -> Option<String> {
    let parent = node.parent()?;
    if !matches!(
        parent.kind(),
        "jsx_opening_element" | "jsx_self_closing_element"
    ) {
        return None;
    }
    let element_name = jsx_element_name(parent, source)?;
    if !is_probable_jsx_component_name(&element_name) {
        return None;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == "property_identifier")
        .and_then(|child| node_source_text(child, source))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

fn collect_tsx_jsx_usage_edges(tree: &Tree, source: &str) -> Vec<ManualEdgeSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        let Some(source_name) = tsx_owner_name(node, source) else {
            return;
        };
        let line = Some(node.start_position().row as u32 + 1);
        match node.kind() {
            "jsx_self_closing_element" | "jsx_opening_element" => {
                if let Some(name) = jsx_element_name(node, source)
                    .filter(|name| is_probable_jsx_component_name(name))
                {
                    edges.push(ManualEdgeSpec {
                        source_name,
                        target_name: name,
                        kind: EdgeKind::CALL,
                        line,
                    });
                }
            }
            "jsx_attribute" => {
                if let Some(name) = jsx_attribute_target_name(node, source) {
                    edges.push(ManualEdgeSpec {
                        source_name,
                        target_name: name,
                        kind: EdgeKind::USAGE,
                        line,
                    });
                }
            }
            _ => {}
        }
    });
    edges
}

fn is_javascript_like_language(language_name: &str) -> bool {
    matches!(language_name, "javascript" | "typescript" | "tsx")
}

fn js_identifier_target_name(node: TsNode<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" | "type_identifier" => node_source_text(node, source)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty()),
        "member_expression" => node
            .child_by_field_name("property")
            .and_then(|property| js_identifier_target_name(property, source)),
        _ => None,
    }
}

fn js_member_object_identifier(node: TsNode<'_>, source: &str) -> Option<String> {
    if node.kind() != "member_expression" {
        return None;
    }
    let object = node.child_by_field_name("object")?;
    match object.kind() {
        "identifier" => node_source_text(object, source)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty()),
        _ => None,
    }
}

fn js_member_property_name(node: TsNode<'_>, source: &str) -> Option<String> {
    if node.kind() != "member_expression" {
        return None;
    }
    node.child_by_field_name("property")
        .and_then(|property| node_source_text(property, source))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

fn js_new_expression_constructor_name(node: TsNode<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("constructor")
        .and_then(|constructor| js_identifier_target_name(constructor, source))
        .or_else(|| {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find_map(|child| js_identifier_target_name(child, source))
        })
}

fn collect_javascript_static_call_edges(tree: &Tree, source: &str) -> Vec<ManualEdgeSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        let Some(source_name) = tsx_owner_name(node, source) else {
            return;
        };
        let line = Some(node.start_position().row as u32 + 1);
        match node.kind() {
            "new_expression" => {
                if let Some(target_name) = js_new_expression_constructor_name(node, source) {
                    edges.push(ManualEdgeSpec {
                        source_name,
                        target_name,
                        kind: EdgeKind::CALL,
                        line,
                    });
                }
            }
            "call_expression" => {
                let Some(function_node) = node.child_by_field_name("function") else {
                    return;
                };
                let Some(property_name) = js_member_property_name(function_node, source) else {
                    return;
                };
                if !matches!(property_name.as_str(), "call" | "apply" | "bind") {
                    return;
                }
                if let Some(target_name) = js_member_object_identifier(function_node, source) {
                    edges.push(ManualEdgeSpec {
                        source_name,
                        target_name,
                        kind: EdgeKind::CALL,
                        line,
                    });
                }
            }
            _ => {}
        }
    });
    edges
}

fn js_like_callable_source_name(node: TsNode<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "function_declaration" | "method_definition" => declaration_name(node, source),
        "arrow_function" => node
            .parent()
            .and_then(|parent| tsx_callable_binding_name(parent, source)),
        "function_expression" => node
            .parent()
            .filter(|parent| {
                parent.kind() == "assignment_expression"
                    && parent
                        .child_by_field_name("right")
                        .is_some_and(|right| same_ts_span(right, node))
            })
            .and_then(|assignment| assignment.child_by_field_name("left"))
            .filter(|left| left.kind() == "member_expression")
            .and_then(|left| normalized_receiver_variable(left, source)),
        _ => None,
    }
}

fn js_ts_visible_local_type_name(
    callable: TsNode<'_>,
    before_node: TsNode<'_>,
    owner_name: &str,
    source: &str,
) -> bool {
    let mut found = false;
    walk_tree_nodes(callable, &mut |node| {
        if found
            || !matches!(
                node.kind(),
                "class_declaration"
                    | "interface_declaration"
                    | "type_alias_declaration"
                    | "enum_declaration"
            )
            || !receiver_call_belongs_to_callable(node, callable)
            || node.end_byte() > before_node.start_byte()
            || !js_ts_local_binding_visible_at_call(node, before_node)
        {
            return;
        }
        if declaration_name(node, source).as_deref() == Some(owner_name) {
            found = true;
        }
    });
    found
}

fn js_ts_local_binding_visible_at_call(binding: TsNode<'_>, call_node: TsNode<'_>) -> bool {
    let Some(binding_scope) = js_ts_lexical_scope(binding) else {
        return false;
    };
    let Some(call_scope) = js_ts_lexical_scope(call_node) else {
        return false;
    };
    node_is_same_or_ancestor(binding_scope, call_scope)
}

fn js_ts_lexical_scope(node: TsNode<'_>) -> Option<TsNode<'_>> {
    enclosing_node_with_kind(node, &["statement_block", "block", "program"])
}

fn rust_macro_owner_name(mut node: TsNode<'_>, source: &str) -> Option<String> {
    while let Some(parent) = node.parent() {
        if parent.kind() == "function_item" {
            return parent
                .child_by_field_name("name")
                .and_then(|name| node_source_text(name, source))
                .map(|name| name.trim().to_string());
        }
        node = parent;
    }
    None
}

fn rust_macro_target_name(node: TsNode<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("macro")
        .and_then(|macro_node| node_source_text(macro_node, source))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

fn collect_rust_macro_call_edges(tree: &Tree, source: &str) -> Vec<ManualEdgeSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "macro_invocation" {
            return;
        }
        let Some(source_name) = rust_macro_owner_name(node, source) else {
            return;
        };
        let Some(target_name) = rust_macro_target_name(node, source) else {
            return;
        };
        edges.push(ManualEdgeSpec {
            source_name,
            target_name,
            kind: EdgeKind::CALL,
            line: Some(node.start_position().row as u32 + 1),
        });
    });
    edges
}

fn collect_ruby_bare_call_edges(tree: &Tree, source: &str) -> Vec<ManualEdgeSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |callable| {
        if !matches!(callable.kind(), "method" | "singleton_method") {
            return;
        }
        let Some(source_name) = declaration_name(callable, source) else {
            return;
        };
        let local_bindings = collect_ruby_local_binding_names(callable, source);
        walk_tree_nodes(callable, &mut |node| {
            if !matches!(node.kind(), "identifier" | "constant") || !is_ruby_bare_call_site(node) {
                return;
            }
            let Some(target_name) = trimmed_node_text(node, source) else {
                return;
            };
            if local_bindings.contains(&target_name) {
                return;
            }
            edges.push(ManualEdgeSpec {
                source_name: source_name.clone(),
                target_name,
                kind: EdgeKind::CALL,
                line: Some(node.start_position().row as u32 + 1),
            });
        });
    });
    edges
}

fn collect_ruby_local_binding_names(callable: TsNode<'_>, source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    walk_tree_nodes(callable, &mut |node| {
        if !matches!(node.kind(), "identifier" | "constant") {
            return;
        }
        let Some(parent) = node.parent() else {
            return;
        };
        let is_binding = match parent.kind() {
            "assignment" => parent
                .child_by_field_name("left")
                .map(|left| same_ts_span(left, node))
                .unwrap_or(false),
            "parameters" | "method_parameters" | "optional_parameter" | "keyword_parameter" => true,
            _ => false,
        };
        if !is_binding {
            return;
        }
        if let Some(name) = trimmed_node_text(node, source) {
            names.insert(name);
        }
    });
    names
}

fn is_ruby_bare_call_site(node: TsNode<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if matches!(
        parent.kind(),
        "method"
            | "singleton_method"
            | "class"
            | "module"
            | "assignment"
            | "parameters"
            | "method_parameters"
            | "optional_parameter"
            | "keyword_parameter"
    ) {
        return false;
    }
    if parent.kind() == "call" {
        return false;
    }
    if let Some(name) = parent.child_by_field_name("name")
        && same_ts_span(name, node)
    {
        return false;
    }
    if let Some(left) = parent.child_by_field_name("left")
        && same_ts_span(left, node)
    {
        return false;
    }
    if let Some(receiver) = parent.child_by_field_name("receiver")
        && same_ts_span(receiver, node)
    {
        return false;
    }
    if let Some(method) = parent.child_by_field_name("method")
        && same_ts_span(method, node)
    {
        return false;
    }
    true
}

fn same_ts_span(left: TsNode<'_>, right: TsNode<'_>) -> bool {
    left.start_byte() == right.start_byte() && left.end_byte() == right.end_byte()
}

fn node_matches_name(node: &Node, name: &str) -> bool {
    node.serialized_name == name
        || short_member_name(&node.serialized_name) == name
        || node
            .qualified_name
            .as_deref()
            .map(|qualified_name| {
                qualified_name == name || short_member_name(qualified_name) == name
            })
            .unwrap_or(false)
}

fn runtime_import_binding_target_id(
    node: TsNode<'_>,
    source: &str,
    file_name: &str,
    unique_nodes: &mut HashMap<NodeId, Node>,
    symbol_table: Option<&Arc<SymbolTable>>,
) -> Option<NodeId> {
    let name = node_source_text(node, source)?.trim().to_string();
    if name.is_empty() {
        return None;
    }

    let span = ts_node_graph_span(node);
    if let Some(node_id) = unique_nodes
        .values()
        .filter(|candidate| !matches!(candidate.kind, NodeKind::FILE | NodeKind::MODULE))
        .filter(|candidate| {
            candidate.start_line == Some(span.start_line)
                && candidate.start_col == Some(span.start_col)
                && candidate.end_line == Some(span.end_line)
                && candidate.end_col == Some(span.end_col)
        })
        .filter(|candidate| node_matches_name(candidate, &name))
        .min_by_key(|candidate| candidate.id)
        .map(|candidate| candidate.id)
    {
        return Some(node_id);
    }

    let canonical_seed = format!(
        "{file_name}:{name}:runtime_import_binding:{}:{}",
        span.start_line, span.start_col
    );
    let node_id = NodeId(generate_id(&canonical_seed));
    unique_nodes.entry(node_id).or_insert_with(|| Node {
        id: node_id,
        kind: NodeKind::UNKNOWN,
        serialized_name: name.clone(),
        start_line: Some(span.start_line),
        start_col: Some(span.start_col),
        end_line: Some(span.end_line),
        end_col: Some(span.end_col),
        ..Default::default()
    });
    if let Some(table) = symbol_table {
        table.insert(node_id.0, NodeKind::UNKNOWN);
    }
    Some(node_id)
}

fn runtime_import_binding_node_id(
    node: TsNode<'_>,
    source: &str,
    file_name: &str,
    unique_nodes: &mut HashMap<NodeId, Node>,
    symbol_table: Option<&Arc<SymbolTable>>,
) -> Option<NodeId> {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "parenthesized_expression"
            | "await_expression"
            | "as_expression"
            | "satisfies_expression"
            | "type_assertion"
            | "non_null_expression" => {
                current = parent;
            }
            "variable_declarator" => {
                return parent.child_by_field_name("name").and_then(|binding| {
                    runtime_import_binding_target_id(
                        binding,
                        source,
                        file_name,
                        unique_nodes,
                        symbol_table,
                    )
                });
            }
            "assignment_expression" => {
                return parent.child_by_field_name("left").and_then(|binding| {
                    runtime_import_binding_target_id(
                        binding,
                        source,
                        file_name,
                        unique_nodes,
                        symbol_table,
                    )
                });
            }
            _ => return None,
        }
    }
    None
}

fn collect_javascript_binding_identifier_nodes<'tree>(
    node: TsNode<'tree>,
    bindings: &mut Vec<TsNode<'tree>>,
) {
    match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => bindings.push(node),
        "pair_pattern" => {
            if let Some(value) = node.child_by_field_name("value") {
                collect_javascript_binding_identifier_nodes(value, bindings);
            }
        }
        "assignment_pattern" => {
            if let Some(left) = node.child_by_field_name("left") {
                collect_javascript_binding_identifier_nodes(left, bindings);
            }
        }
        "rest_pattern" => {
            if let Some(argument) = node.child_by_field_name("argument") {
                collect_javascript_binding_identifier_nodes(argument, bindings);
            }
        }
        "required_parameter" | "optional_parameter" => {
            if let Some(pattern) = node
                .child_by_field_name("pattern")
                .or_else(|| node.child_by_field_name("name"))
                .or_else(|| node.named_child(0))
            {
                collect_javascript_binding_identifier_nodes(pattern, bindings);
            }
        }
        "object_pattern" | "array_pattern" | "formal_parameters" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_javascript_binding_identifier_nodes(child, bindings);
            }
        }
        _ => {}
    }
}

fn javascript_runtime_import_bindings<'tree>(
    tree: &'tree Tree,
    source: &str,
    import_call: TsNode<'tree>,
) -> Vec<JavaScriptRuntimeImportBinding<'tree>> {
    let mut current = import_call;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            "parenthesized_expression"
            | "await_expression"
            | "as_expression"
            | "satisfies_expression"
            | "type_assertion"
            | "non_null_expression" => current = parent,
            "member_expression"
                if parent
                    .child_by_field_name("object")
                    .is_some_and(|object| same_ts_span(object, current)) =>
            {
                current = parent;
            }
            "variable_declarator"
                if parent
                    .child_by_field_name("value")
                    .is_some_and(|value| same_ts_span(value, current)) =>
            {
                let mut bindings = Vec::new();
                if let Some(name) = parent.child_by_field_name("name") {
                    collect_javascript_binding_identifier_nodes(name, &mut bindings);
                }
                let activation_end_byte = parent.parent().unwrap_or(parent).end_byte();
                return bindings
                    .into_iter()
                    .map(|declaration_binding| JavaScriptRuntimeImportBinding {
                        declaration_binding,
                        activation_end_byte,
                    })
                    .collect();
            }
            "assignment_expression"
                if parent
                    .child_by_field_name("right")
                    .is_some_and(|right| same_ts_span(right, current)) =>
            {
                let Some(left) = parent
                    .child_by_field_name("left")
                    .filter(|left| left.kind() == "identifier")
                else {
                    return Vec::new();
                };
                let Some(name) = trimmed_node_text(left, source) else {
                    return Vec::new();
                };
                let mut declarations =
                    collect_javascript_binding_occurrences(tree.root_node(), source, &name)
                        .into_iter()
                        .filter(|occurrence| ts_node_contains(occurrence.scope, parent))
                        .collect::<Vec<_>>();
                declarations.sort_by_key(|occurrence| {
                    (
                        occurrence.scope.start_byte(),
                        occurrence.binding.start_byte(),
                    )
                });
                let Some(declaration) = declarations.pop() else {
                    return Vec::new();
                };
                if declarations
                    .iter()
                    .any(|other| same_ts_span(other.scope, declaration.scope))
                {
                    return Vec::new();
                }
                if declaration.binding.start_byte() >= parent.start_byte()
                    || javascript_variable_declarator_for_binding(declaration.binding)
                        .is_none_or(|declarator| declarator.child_by_field_name("value").is_some())
                {
                    return Vec::new();
                }
                return vec![JavaScriptRuntimeImportBinding {
                    declaration_binding: declaration.binding,
                    activation_end_byte: parent.end_byte(),
                }];
            }
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

fn javascript_variable_declarator_for_binding(mut binding: TsNode<'_>) -> Option<TsNode<'_>> {
    while let Some(parent) = binding.parent() {
        if parent.kind() == "variable_declarator" {
            return Some(parent);
        }
        if matches!(parent.kind(), "program" | "statement_block") {
            return None;
        }
        binding = parent;
    }
    None
}

fn javascript_callable_scope(node: TsNode<'_>) -> bool {
    matches!(
        node.kind(),
        "function_declaration"
            | "function_expression"
            | "generator_function_declaration"
            | "generator_function"
            | "arrow_function"
            | "method_definition"
    )
}

fn javascript_nearest_scope<'tree>(
    mut node: TsNode<'tree>,
    lexical: bool,
) -> Option<TsNode<'tree>> {
    while let Some(parent) = node.parent() {
        if parent.kind() == "program"
            || javascript_callable_scope(parent)
            || (lexical
                && matches!(
                    parent.kind(),
                    "statement_block" | "catch_clause" | "for_statement" | "for_in_statement"
                ))
        {
            return Some(parent);
        }
        node = parent;
    }
    None
}

#[derive(Debug, Clone, Copy)]
struct JavaScriptBindingOccurrence<'tree> {
    binding: TsNode<'tree>,
    scope: TsNode<'tree>,
}

#[derive(Debug, Clone, Copy)]
struct JavaScriptRuntimeImportBinding<'tree> {
    declaration_binding: TsNode<'tree>,
    activation_end_byte: usize,
}

fn collect_javascript_binding_occurrences<'tree>(
    root: TsNode<'tree>,
    source: &str,
    name: &str,
) -> Vec<JavaScriptBindingOccurrence<'tree>> {
    let mut occurrences = Vec::new();
    walk_tree_nodes(root, &mut |node| {
        match node.kind() {
            "variable_declarator" => {
                let Some(pattern) = node.child_by_field_name("name") else {
                    return;
                };
                let lexical = node
                    .parent()
                    .is_some_and(|parent| parent.kind() != "variable_declaration");
                let Some(scope) = javascript_nearest_scope(node, lexical) else {
                    return;
                };
                let mut bindings = Vec::new();
                collect_javascript_binding_identifier_nodes(pattern, &mut bindings);
                occurrences.extend(bindings.into_iter().filter_map(|binding| {
                    (trimmed_node_text(binding, source).as_deref() == Some(name))
                        .then_some(JavaScriptBindingOccurrence { binding, scope })
                }));
            }
            "function_declaration" | "generator_function_declaration" | "class_declaration" => {
                if let Some(binding) = node.child_by_field_name("name")
                    && trimmed_node_text(binding, source).as_deref() == Some(name)
                    && let Some(scope) = javascript_nearest_scope(node, true)
                {
                    occurrences.push(JavaScriptBindingOccurrence { binding, scope });
                }
            }
            "function_expression" | "generator_function" | "class" => {
                if let Some(binding) = node.child_by_field_name("name")
                    && trimmed_node_text(binding, source).as_deref() == Some(name)
                {
                    occurrences.push(JavaScriptBindingOccurrence {
                        binding,
                        scope: node,
                    });
                }
            }
            "catch_clause" => {
                let Some(parameter) = node.child_by_field_name("parameter") else {
                    return;
                };
                let mut bindings = Vec::new();
                collect_javascript_binding_identifier_nodes(parameter, &mut bindings);
                occurrences.extend(bindings.into_iter().filter_map(|binding| {
                    (trimmed_node_text(binding, source).as_deref() == Some(name)).then_some(
                        JavaScriptBindingOccurrence {
                            binding,
                            scope: node,
                        },
                    )
                }));
            }
            _ => {}
        }

        if javascript_callable_scope(node) {
            let parameters = node
                .child_by_field_name("parameters")
                .or_else(|| node.child_by_field_name("parameter"));
            let Some(parameters) = parameters else {
                return;
            };
            let mut bindings = Vec::new();
            collect_javascript_binding_identifier_nodes(parameters, &mut bindings);
            occurrences.extend(bindings.into_iter().filter_map(|binding| {
                (trimmed_node_text(binding, source).as_deref() == Some(name)).then_some(
                    JavaScriptBindingOccurrence {
                        binding,
                        scope: node,
                    },
                )
            }));
        }
    });
    occurrences
}

fn ts_node_contains(outer: TsNode<'_>, inner: TsNode<'_>) -> bool {
    outer.start_byte() <= inner.start_byte() && outer.end_byte() >= inner.end_byte()
}

fn javascript_binding_has_prior_write(
    root: TsNode<'_>,
    source: &str,
    declaration_binding: TsNode<'_>,
    after_byte: usize,
    proof_node: TsNode<'_>,
) -> bool {
    let Some(name) = trimmed_node_text(declaration_binding, source) else {
        return true;
    };
    let occurrences = collect_javascript_binding_occurrences(root, source, &name);
    let Some(declaration) = occurrences
        .iter()
        .find(|occurrence| same_ts_span(occurrence.binding, declaration_binding))
    else {
        return true;
    };
    let declaration_callable = javascript_enclosing_callable(declaration_binding);
    let proof_callable = javascript_enclosing_callable(proof_node);

    let mut written = false;
    walk_tree_nodes(root, &mut |node| {
        if written || !matches!(node.kind(), "assignment_expression" | "update_expression") {
            return;
        }
        let target = if node.kind() == "assignment_expression" {
            node.child_by_field_name("left")
        } else {
            node.named_child(0)
        };
        let Some(target) = target else {
            return;
        };
        let mut bindings = Vec::new();
        collect_javascript_binding_identifier_nodes(target, &mut bindings);
        if !bindings
            .into_iter()
            .any(|binding| trimmed_node_text(binding, source).as_deref() == Some(name.as_str()))
        {
            return;
        }
        let write_callable = javascript_enclosing_callable(node);
        let unordered_nested_write = write_callable.is_some_and(|write_callable| {
            !declaration_callable.is_some_and(|owner| same_ts_span(owner, write_callable))
                && !proof_callable.is_some_and(|owner| same_ts_span(owner, write_callable))
                && ts_node_contains(declaration.scope, write_callable)
        });
        let cyclic_write = javascript_nodes_share_enclosing_cycle(node, proof_node);
        if !unordered_nested_write
            && !cyclic_write
            && (node.start_byte() < after_byte || node.end_byte() >= proof_node.start_byte())
        {
            return;
        }
        if !ts_node_contains(declaration.scope, node)
            || occurrences.iter().any(|occurrence| {
                !same_ts_span(occurrence.binding, declaration.binding)
                    && ts_node_contains(declaration.scope, occurrence.scope)
                    && ts_node_contains(occurrence.scope, node)
            })
        {
            return;
        }
        written = true;
    });
    written
}

fn javascript_nodes_share_enclosing_cycle(left: TsNode<'_>, right: TsNode<'_>) -> bool {
    let mut current = left;
    while let Some(parent) = current.parent() {
        if matches!(
            parent.kind(),
            "for_statement" | "for_in_statement" | "while_statement" | "do_statement"
        ) && ts_node_contains(parent, right)
        {
            return true;
        }
        current = parent;
    }
    false
}

fn javascript_enclosing_callable(mut node: TsNode<'_>) -> Option<TsNode<'_>> {
    while let Some(parent) = node.parent() {
        if javascript_callable_scope(parent) {
            return Some(parent);
        }
        node = parent;
    }
    None
}

fn javascript_runtime_import_binding_visible_at_call(
    tree: &Tree,
    source: &str,
    import_binding: TsNode<'_>,
    activation_end_byte: usize,
    call_callee: TsNode<'_>,
) -> bool {
    let Some(name) = trimmed_node_text(import_binding, source) else {
        return false;
    };
    if call_callee.start_byte() <= activation_end_byte {
        return false;
    }
    let occurrences = collect_javascript_binding_occurrences(tree.root_node(), source, &name);
    let Some(import_occurrence) = occurrences
        .iter()
        .find(|occurrence| same_ts_span(occurrence.binding, import_binding))
    else {
        return false;
    };
    if !ts_node_contains(import_occurrence.scope, call_callee) {
        return false;
    }
    if occurrences.iter().any(|occurrence| {
        !same_ts_span(occurrence.binding, import_binding)
            && ts_node_contains(import_occurrence.scope, occurrence.scope)
            && ts_node_contains(occurrence.scope, call_callee)
    }) {
        return false;
    }

    !javascript_binding_has_prior_write(
        tree.root_node(),
        source,
        import_binding,
        activation_end_byte,
        call_callee,
    )
}

fn javascript_runtime_import_bare_call_target_spans(
    tree: &Tree,
    source: &str,
    import_binding: TsNode<'_>,
    activation_end_byte: usize,
) -> Vec<GraphNodeSpan> {
    let Some(name) = trimmed_node_text(import_binding, source) else {
        return Vec::new();
    };
    let mut spans = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "call_expression" {
            return;
        }
        let Some(callee) = node.child_by_field_name("function") else {
            return;
        };
        if callee.kind() != "identifier"
            || trimmed_node_text(callee, source).as_deref() != Some(name.as_str())
            || !javascript_runtime_import_binding_visible_at_call(
                tree,
                source,
                import_binding,
                activation_end_byte,
                callee,
            )
        {
            return;
        }
        spans.push(ts_node_graph_span(callee));
    });
    spans
}

fn collect_runtime_import_specs(
    language_name: &str,
    file_name: &str,
    tree: &Tree,
    source: &str,
    unique_nodes: &mut HashMap<NodeId, Node>,
    symbol_table: Option<&Arc<SymbolTable>>,
) -> Vec<RuntimeImportSpec> {
    if language_name == "bash" {
        return collect_bash_source_import_specs(
            file_name,
            tree,
            source,
            unique_nodes,
            symbol_table,
        );
    }
    if matches!(language_name, "javascript" | "typescript" | "tsx") {
        return collect_javascript_runtime_import_specs(
            file_name,
            tree,
            source,
            unique_nodes,
            symbol_table,
        );
    }
    if language_name == "ruby" {
        return collect_ruby_runtime_import_specs(
            file_name,
            tree,
            source,
            unique_nodes,
            symbol_table,
        );
    }

    Vec::new()
}

fn collect_javascript_runtime_import_specs(
    file_name: &str,
    tree: &Tree,
    source: &str,
    unique_nodes: &mut HashMap<NodeId, Node>,
    symbol_table: Option<&Arc<SymbolTable>>,
) -> Vec<RuntimeImportSpec> {
    let mut specs = Vec::new();
    let mut exact_bindings = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "call_expression" {
            return;
        }
        let Some(function_node) = node.child_by_field_name("function") else {
            return;
        };
        let Some(callee_name) =
            node_source_text(function_node, source).map(|name| name.trim().to_string())
        else {
            return;
        };
        if callee_name != "require" && callee_name != "import" {
            return;
        }
        let Some(arguments) = node.child_by_field_name("arguments") else {
            return;
        };
        let mut cursor = arguments.walk();
        let Some(module_node) =
            arguments
                .named_children(&mut cursor)
                .find(|child| match child.kind() {
                    "string" | "string_literal" => true,
                    "template_string" => {
                        let mut template_cursor = child.walk();
                        !child
                            .named_children(&mut template_cursor)
                            .any(|part| part.kind() == "template_substitution")
                    }
                    _ => false,
                })
        else {
            return;
        };
        let Some(module_name) = node_source_text(module_node, source)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
        else {
            return;
        };
        let start = module_node.start_position();
        let end = module_node.end_position();
        let line = start.row as u32 + 1;
        let suppress_line = node.start_position().row as u32 + 1;
        let suppress_start_col = function_node.start_position().column as u32 + 1;
        let canonical_seed = format!("{file_name}:{module_name}:{line}");
        let module_node_id = NodeId(generate_id(&canonical_seed));
        unique_nodes.entry(module_node_id).or_insert_with(|| Node {
            id: module_node_id,
            kind: NodeKind::MODULE,
            serialized_name: module_name,
            start_line: Some(line),
            start_col: Some(start.column as u32 + 1),
            end_line: Some(end.row as u32 + 1),
            end_col: Some(end.column as u32 + 1),
            ..Default::default()
        });
        if let Some(table) = symbol_table {
            table.insert(module_node_id.0, NodeKind::MODULE);
        }
        let bindings = javascript_runtime_import_bindings(tree, source, node);
        if bindings.is_empty() {
            specs.push(RuntimeImportSpec {
                binding_node_id: runtime_import_binding_node_id(
                    node,
                    source,
                    file_name,
                    unique_nodes,
                    symbol_table,
                ),
                module_node_id,
                line,
                suppress_line,
                suppress_start_col,
                suppress_callee_name: callee_name,
                exact_bare_call_target_spans: Vec::new(),
            });
            return;
        }
        for binding in bindings {
            let binding_node_id = runtime_import_binding_target_id(
                binding.declaration_binding,
                source,
                file_name,
                unique_nodes,
                symbol_table,
            );
            let spec_index = specs.len();
            specs.push(RuntimeImportSpec {
                binding_node_id,
                module_node_id,
                line,
                suppress_line,
                suppress_start_col,
                suppress_callee_name: callee_name.clone(),
                exact_bare_call_target_spans: Vec::new(),
            });
            exact_bindings.push((spec_index, binding));
        }
    });
    for (spec_index, binding) in exact_bindings {
        specs[spec_index].exact_bare_call_target_spans =
            javascript_runtime_import_bare_call_target_spans(
                tree,
                source,
                binding.declaration_binding,
                binding.activation_end_byte,
            );
    }
    specs
}

fn collect_ruby_runtime_import_specs(
    file_name: &str,
    tree: &Tree,
    source: &str,
    unique_nodes: &mut HashMap<NodeId, Node>,
    symbol_table: Option<&Arc<SymbolTable>>,
) -> Vec<RuntimeImportSpec> {
    let mut specs = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "call" {
            return;
        }
        if node.child_by_field_name("receiver").is_some() {
            return;
        }
        let Some(method_node) = node.child_by_field_name("method") else {
            return;
        };
        let Some(callee_name) =
            node_source_text(method_node, source).map(|name| name.trim().to_string())
        else {
            return;
        };
        if callee_name != "require" && callee_name != "require_relative" {
            return;
        }
        let Some(arguments) = node.child_by_field_name("arguments") else {
            return;
        };
        let mut cursor = arguments.walk();
        let argument_nodes = arguments.named_children(&mut cursor).collect::<Vec<_>>();
        let [module_node] = argument_nodes.as_slice() else {
            return;
        };
        if module_node.kind() != "string" {
            return;
        }
        let Some(module_name) = node_source_text(*module_node, source)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .filter(|name| quoted_literal_surface(name).is_some())
            .filter(|name| !name.contains("#{"))
        else {
            return;
        };
        let start = module_node.start_position();
        let end = module_node.end_position();
        let line = start.row as u32 + 1;
        let suppress_line = node.start_position().row as u32 + 1;
        let suppress_start_col = method_node.start_position().column as u32 + 1;
        let canonical_seed = format!("{file_name}:{module_name}:{line}");
        let module_node_id = NodeId(generate_id(&canonical_seed));
        unique_nodes.entry(module_node_id).or_insert_with(|| Node {
            id: module_node_id,
            kind: NodeKind::MODULE,
            serialized_name: module_name,
            start_line: Some(line),
            start_col: Some(start.column as u32 + 1),
            end_line: Some(end.row as u32 + 1),
            end_col: Some(end.column as u32 + 1),
            ..Default::default()
        });
        if let Some(table) = symbol_table {
            table.insert(module_node_id.0, NodeKind::MODULE);
        }
        specs.push(RuntimeImportSpec {
            binding_node_id: None,
            module_node_id,
            line,
            suppress_line,
            suppress_start_col,
            suppress_callee_name: callee_name,
            exact_bare_call_target_spans: Vec::new(),
        });
    });
    specs
}

fn collect_bash_source_import_specs(
    file_name: &str,
    tree: &Tree,
    source: &str,
    unique_nodes: &mut HashMap<NodeId, Node>,
    symbol_table: Option<&Arc<SymbolTable>>,
) -> Vec<RuntimeImportSpec> {
    let mut specs = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "command" {
            return;
        }
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        let Some(callee_name) =
            node_source_text(name_node, source).map(|name| name.trim().to_string())
        else {
            return;
        };
        if callee_name != "source" && callee_name != "." {
            return;
        }

        let mut cursor = node.walk();
        let Some(module_node) = node.named_children(&mut cursor).find(|child| {
            child.start_byte() >= name_node.end_byte()
                && matches!(
                    child.kind(),
                    "word" | "raw_string" | "string" | "concatenation"
                )
        }) else {
            return;
        };
        let Some(module_name) = node_source_text(module_node, source)
            .and_then(|name| normalize_static_shell_module_name(&name))
        else {
            return;
        };

        let start = module_node.start_position();
        let end = module_node.end_position();
        let line = start.row as u32 + 1;
        let suppress_line = node.start_position().row as u32 + 1;
        let suppress_start_col = name_node.start_position().column as u32 + 1;
        let canonical_seed = format!("{file_name}:{module_name}:{line}");
        let module_node_id = NodeId(generate_id(&canonical_seed));
        unique_nodes.entry(module_node_id).or_insert_with(|| Node {
            id: module_node_id,
            kind: NodeKind::MODULE,
            serialized_name: module_name,
            start_line: Some(line),
            start_col: Some(start.column as u32 + 1),
            end_line: Some(end.row as u32 + 1),
            end_col: Some(end.column as u32 + 1),
            ..Default::default()
        });
        if let Some(table) = symbol_table {
            table.insert(module_node_id.0, NodeKind::MODULE);
        }
        specs.push(RuntimeImportSpec {
            binding_node_id: None,
            module_node_id,
            line,
            suppress_line,
            suppress_start_col,
            suppress_callee_name: callee_name,
            exact_bare_call_target_spans: Vec::new(),
        });
    });
    specs
}

fn normalize_static_shell_module_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains('$')
        || trimmed.contains('*')
        || trimmed.contains('?')
        || trimmed.contains('`')
    {
        return None;
    }

    let unquoted = if trimmed.len() >= 2 {
        let bytes = trimmed.as_bytes();
        if (bytes.first() == Some(&b'"') && bytes.last() == Some(&b'"'))
            || (bytes.first() == Some(&b'\'') && bytes.last() == Some(&b'\''))
        {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    (!unquoted.trim().is_empty()).then(|| unquoted.trim().to_string())
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
                    .map(|qualified_name| {
                        qualified_name == name || short_member_name(qualified_name) == name
                    })
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

#[allow(clippy::too_many_arguments)]
fn append_manual_type_argument_edges(
    language_name: &str,
    tree: &Tree,
    source: &str,
    unique_nodes: &HashMap<NodeId, Node>,
    file_id: NodeId,
    result_edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
    callsite_ordinals: &mut HashMap<(NodeId, Option<u32>), u32>,
) {
    let specs = match language_name {
        "rust" => collect_rust_generic_type_argument_edges(tree, source),
        "cpp" => collect_cpp_template_type_argument_edges(tree, source),
        _ => Vec::new(),
    };

    for spec in specs {
        let source_id = match spec.kind {
            EdgeKind::CALL => unique_node_id_by_name(unique_nodes, &spec.source_name, |kind| {
                matches!(
                    kind,
                    NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
                )
            }),
            EdgeKind::TYPE_ARGUMENT if language_name == "rust" => {
                unique_node_id_by_name(unique_nodes, &spec.source_name, |kind| {
                    matches!(
                        kind,
                        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
                    )
                })
            }
            _ => unique_node_id_by_name(unique_nodes, &spec.source_name, is_type_like_kind),
        };
        let Some(source_id) = source_id else {
            continue;
        };
        let target_id = match spec.kind {
            EdgeKind::CALL => unique_node_id_by_name(unique_nodes, &spec.target_name, |kind| {
                matches!(
                    kind,
                    NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
                )
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
            let key = (edge.target, edge.line);
            let next = callsite_ordinals.entry(key).or_insert(0);
            *next = next.saturating_add(1);
            ensure_callsite_identity(&mut edge, Some(*next));
        }
        if !edge_keys.insert(edge_dedup_key(&edge, flags)) {
            continue;
        }
        edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
        result_edges.push(edge);
    }
}

#[allow(clippy::too_many_arguments)]
fn append_manual_usage_edges(
    language_name: &str,
    is_tsx_file: bool,
    tree: &Tree,
    source: &str,
    unique_nodes: &HashMap<NodeId, Node>,
    file_id: NodeId,
    result_edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
    callsite_ordinals: &mut HashMap<(NodeId, Option<u32>), u32>,
) {
    let mut specs = Vec::new();
    if is_tsx_file {
        specs.extend(collect_tsx_jsx_usage_edges(tree, source));
    }
    if is_javascript_like_language(language_name) {
        specs.extend(collect_javascript_static_call_edges(tree, source));
    }
    if language_name == "rust" {
        specs.extend(collect_rust_macro_call_edges(tree, source));
    }
    if language_name == "python" {
        specs.extend(languages::python::decorator_call_specs(tree, source));
    }
    if language_name == "ruby" {
        specs.extend(collect_ruby_bare_call_edges(tree, source));
    }
    if specs.is_empty() {
        return;
    }

    for spec in specs {
        let Some(source_id) = unique_node_id_by_name(unique_nodes, &spec.source_name, |kind| {
            if language_name == "python" {
                matches!(
                    kind,
                    NodeKind::CLASS | NodeKind::FUNCTION | NodeKind::METHOD
                )
            } else {
                matches!(kind, NodeKind::FUNCTION | NodeKind::METHOD)
            }
        }) else {
            continue;
        };
        let target_id = match spec.kind {
            EdgeKind::CALL => unique_node_id_by_name(unique_nodes, &spec.target_name, |kind| {
                if is_tsx_file
                    || language_name == "python"
                    || is_javascript_like_language(language_name)
                {
                    matches!(
                        kind,
                        NodeKind::CLASS
                            | NodeKind::FUNCTION
                            | NodeKind::METHOD
                            | NodeKind::MACRO
                            | NodeKind::UNKNOWN
                    )
                } else {
                    matches!(
                        kind,
                        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
                    )
                }
            }),
            _ => unique_node_id_by_name(unique_nodes, &spec.target_name, |kind| {
                matches!(
                    kind,
                    NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::FIELD
                )
            }),
        };
        let Some(target_id) = target_id else {
            continue;
        };
        if is_tsx_file
            && result_edges.iter().any(|edge| {
                edge.source == source_id
                    && edge.target == target_id
                    && edge.kind == spec.kind
                    && edge.line == spec.line
            })
        {
            continue;
        }

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
            let key = (edge.target, edge.line);
            let next = callsite_ordinals.entry(key).or_insert(0);
            *next = next.saturating_add(1);
            ensure_callsite_identity(&mut edge, Some(*next));
        }
        if !edge_keys.insert(edge_dedup_key(&edge, flags)) {
            continue;
        }
        edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
        result_edges.push(edge);
    }
}

fn language_precise_call_specs(
    language_name: &str,
    tree: &Tree,
    source: &str,
) -> Vec<ManualPreciseCallSpec> {
    // Dart is the only language with a precise-call collector, so
    // `LanguageExtraction` has no field for one; the arm calls into the
    // migrated module rather than into a body still living here.
    match language_name {
        "dart" => languages::dart::direct_call_specs(tree, source),
        _ => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn append_manual_precise_call_edges(
    language_name: &str,
    tree: &Tree,
    source: &str,
    unique_nodes: &HashMap<NodeId, Node>,
    file_id: NodeId,
    result_edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
    callsite_ordinals: &mut HashMap<(NodeId, Option<u32>), u32>,
) {
    for spec in language_precise_call_specs(language_name, tree, source) {
        let Some(source_id) =
            node_id_by_name_and_span(unique_nodes, &spec.source_name, spec.source_span, |kind| {
                matches!(kind, NodeKind::FUNCTION | NodeKind::METHOD)
            })
        else {
            continue;
        };
        let Some(target_id) = unique_node_id_by_name(unique_nodes, &spec.target_name, |kind| {
            matches!(
                kind,
                NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
            )
        }) else {
            continue;
        };

        remove_generic_call_placeholders(
            unique_nodes,
            result_edges,
            edge_keys,
            flags,
            spec.line,
            None,
            &spec.target_name,
        );

        let mut edge = Edge {
            id: EdgeId(0),
            source: source_id,
            target: target_id,
            kind: EdgeKind::CALL,
            file_node_id: Some(file_id),
            line: spec.line,
            resolved_target: Some(target_id),
            confidence: Some(1.0),
            certainty: Some(ResolutionCertainty::Certain),
            ..Default::default()
        };
        if !flags.legacy_edge_identity {
            let key = (edge.target, edge.line);
            let next = callsite_ordinals.entry(key).or_insert(0);
            *next = next.saturating_add(1);
            ensure_callsite_identity(&mut edge, Some(*next));
        }
        if !edge_keys.insert(edge_dedup_key(&edge, flags)) {
            continue;
        }
        edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
        result_edges.push(edge);
    }
}

fn node_id_by_name_and_span<F>(
    nodes: &HashMap<NodeId, Node>,
    name: &str,
    span: GraphNodeSpan,
    predicate: F,
) -> Option<NodeId>
where
    F: Fn(NodeKind) -> bool,
{
    let mut matches = nodes
        .values()
        .filter(|node| predicate(node.kind))
        .filter(|node| {
            node.start_line == Some(span.start_line)
                && node.start_col == Some(span.start_col)
                && node.end_line == Some(span.end_line)
                && node.end_col == Some(span.end_col)
        })
        .filter(|node| node_matches_name(node, name))
        .collect::<Vec<_>>();
    matches.sort_by_key(|node| node.id);
    matches.first().map(|node| node.id)
}

fn language_member_specs(
    language_name: &str,
    tree: &Tree,
    source: &str,
) -> Vec<ManualMemberEdgeSpec> {
    // Every language with a manual MEMBER-edge collector carries it on its
    // registry row, so an unknown language simply has none.
    languages::extraction_for_language(language_name)
        .and_then(|extraction| extraction.member_edge_specs)
        .map_or_else(Vec::new, |collect| collect(tree, source))
}

struct ManualMemberEdgeContext<'a> {
    specs: &'a [ManualMemberEdgeSpec],
    unique_nodes: &'a HashMap<NodeId, Node>,
    file_id: NodeId,
    flags: IndexFeatureFlags,
}

fn append_manual_member_edges(
    context: ManualMemberEdgeContext<'_>,
    result_edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
) -> HashSet<NodeId> {
    let mut target_ids = HashSet::new();
    for spec in context.specs {
        let Some(source_id) = node_id_by_name_and_span(
            context.unique_nodes,
            &spec.source_name,
            spec.source_span,
            manual_member_source_kind,
        ) else {
            continue;
        };
        let Some(target_id) = node_id_by_name_and_span(
            context.unique_nodes,
            &spec.target_name,
            spec.target_span,
            manual_member_target_kind,
        ) else {
            continue;
        };

        target_ids.insert(target_id);
        let mut edge = Edge {
            id: EdgeId(0),
            source: source_id,
            target: target_id,
            kind: EdgeKind::MEMBER,
            file_node_id: Some(context.file_id),
            line: spec.line,
            certainty: parser_direct_structural_certainty(EdgeKind::MEMBER),
            ..Default::default()
        };
        if !edge_keys.insert(edge_dedup_key(&edge, context.flags)) {
            continue;
        }
        edge.id = EdgeId(generate_edge_id_for_edge(&edge, context.flags));
        result_edges.push(edge);
    }
    target_ids
}

fn apply_go_receiver_method_identities(
    language_name: &str,
    nodes: &mut HashMap<NodeId, Node>,
    specs: &[ManualMemberEdgeSpec],
    local_member_targets: &HashSet<NodeId>,
    canonical_roles: &HashMap<NodeId, CanonicalNodeRole>,
) {
    if language_name != "go" || specs.is_empty() {
        return;
    }

    let mut methods_by_span = HashMap::<(u32, u32, u32, u32, String), Vec<NodeId>>::new();
    for node in nodes.values() {
        count_go_method_identity_work(1);
        if node.kind != NodeKind::METHOD
            || !matches!(
                canonical_roles.get(&node.id),
                Some(
                    CanonicalNodeRole::Definition
                        | CanonicalNodeRole::Declaration
                        | CanonicalNodeRole::ForwardDeclaration
                )
            )
        {
            continue;
        }
        let (Some(start_line), Some(start_col), Some(end_line), Some(end_col)) =
            (node.start_line, node.start_col, node.end_line, node.end_col)
        else {
            continue;
        };
        methods_by_span
            .entry((
                start_line,
                start_col,
                end_line,
                end_col,
                short_member_name(&node.serialized_name).to_string(),
            ))
            .or_default()
            .push(node.id);
    }

    for spec in specs {
        count_go_method_identity_work(1);
        let key = (
            spec.target_span.start_line,
            spec.target_span.start_col,
            spec.target_span.end_line,
            spec.target_span.end_col,
            spec.target_name.clone(),
        );
        let Some([method_id]) = methods_by_span.get(&key).map(Vec::as_slice) else {
            continue;
        };
        if local_member_targets.contains(method_id) {
            continue;
        }
        let Some(method) = nodes.get_mut(method_id) else {
            continue;
        };
        let receiver_qualified = format!("{}.{}", spec.source_name, spec.target_name);
        method.serialized_name = receiver_qualified.clone();
        method.qualified_name = Some(receiver_qualified);
    }
}

fn annotate_python_context_manager_self_return_members(
    tree: &Tree,
    source: &str,
    unique_nodes: &HashMap<NodeId, Node>,
    file_id: NodeId,
    result_edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
) {
    for spec in languages::python::context_manager_self_return_member_specs(tree, source) {
        let Some(source_id) = node_id_by_name_and_span(
            unique_nodes,
            &spec.source_name,
            spec.source_span,
            manual_member_source_kind,
        ) else {
            continue;
        };
        let Some(target_id) =
            node_id_by_name_and_span(unique_nodes, &spec.target_name, spec.target_span, |kind| {
                matches!(kind, NodeKind::FUNCTION | NodeKind::METHOD)
            })
        else {
            continue;
        };
        if let Some(edge) = result_edges.iter_mut().find(|edge| {
            edge.kind == EdgeKind::MEMBER && edge.source == source_id && edge.target == target_id
        }) {
            edge.callsite_identity =
                Some(languages::python::CONTEXT_MANAGER_SELF_RETURN_MEMBER_MARKER.to_string());
            continue;
        }

        let mut edge = Edge {
            id: EdgeId(0),
            source: source_id,
            target: target_id,
            kind: EdgeKind::MEMBER,
            file_node_id: Some(file_id),
            line: spec.line,
            certainty: parser_direct_structural_certainty(EdgeKind::MEMBER),
            ..Default::default()
        };
        edge.callsite_identity =
            Some(languages::python::CONTEXT_MANAGER_SELF_RETURN_MEMBER_MARKER.to_string());
        if !edge_keys.insert(edge_dedup_key(&edge, flags)) {
            continue;
        }
        edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
        result_edges.push(edge);
    }
}

fn manual_member_source_kind(kind: NodeKind) -> bool {
    is_type_like_kind(kind) || matches!(kind, NodeKind::MODULE | NodeKind::NAMESPACE)
}

fn manual_member_target_kind(kind: NodeKind) -> bool {
    kind == NodeKind::METHOD || is_type_like_kind(kind)
}

fn language_receiver_call_specs(
    language_name: &str,
    tree: &Tree,
    source: &str,
) -> Vec<ManualReceiverCallSpec> {
    // Same for receiver-call engines: TSX shares TypeScript's, but it shares
    // it through its registry row rather than through an arm here.
    languages::extraction_for_language(language_name)
        .and_then(|extraction| extraction.receiver_call_specs)
        .map_or_else(Vec::new, |collect| collect(tree, source))
}

#[allow(clippy::too_many_arguments)]
fn append_manual_receiver_call_edges(
    language_name: &str,
    tree: &Tree,
    source: &str,
    unique_nodes: &HashMap<NodeId, Node>,
    file_id: NodeId,
    result_edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
    callsite_ordinals: &mut HashMap<(NodeId, Option<u32>), u32>,
) {
    if language_name == "ruby" {
        annotate_ruby_member_call_placeholders(
            tree,
            source,
            unique_nodes,
            result_edges,
            edge_keys,
            flags,
        );
    }

    let context_manager_alias_callsites = if language_name == "python" {
        languages::python::context_manager_alias_callsites(tree, source)
    } else {
        HashSet::new()
    };
    let member_targets = PreparedMemberTargetIndex::prepare(unique_nodes, result_edges);
    let python_local_owner_lines =
        (language_name == "python").then(|| PythonLocalOwnerLineIndex::prepare(tree, source));

    for spec in language_receiver_call_specs(language_name, tree, source) {
        let extra_callsite_marker = context_manager_alias_callsites
            .contains(&receiver_callsite_key(&spec))
            .then_some(languages::python::CONTEXT_MANAGER_SELF_RETURN_REQUIRED_MARKER);
        let Some(source_id) =
            node_id_by_name_and_span(unique_nodes, &spec.source_name, spec.source_span, |kind| {
                if spec.class_anchored {
                    // The type-anchor arm exists only for flagged specs, so
                    // the FUNCTION|METHOD behaviour of every unflagged spec
                    // is untouched by construction. STRUCT joins CLASS
                    // because C# structs own constructors and primary
                    // constructors too.
                    matches!(kind, NodeKind::CLASS | NodeKind::STRUCT)
                } else {
                    matches!(kind, NodeKind::FUNCTION | NodeKind::METHOD)
                }
            })
        else {
            continue;
        };
        let binding_marker = spec.binding_marker.as_deref();
        if let Some(required_callsite_marker) = spec.required_callsite_marker {
            // A marker-override spec (PHP construction) annotates the rule
            // file's own placeholder and nothing else: no in-file owner+method
            // lookup, no fallback placeholder edge. A `new` site whose
            // placeholder is missing therefore stays unannotated and fails
            // closed at resolution.
            annotate_receiver_call_placeholder_owner(
                unique_nodes,
                result_edges,
                edge_keys,
                flags,
                ReceiverPlaceholderAnnotation {
                    line: spec.line,
                    method_col: spec.method_col,
                    method_name: &spec.method_name,
                    owner_name: &spec.owner_name,
                    owner_module: spec.owner_module.as_deref(),
                    extra_callsite_marker,
                    binding_marker,
                },
                Some(required_callsite_marker),
            );
            continue;
        }
        if extra_callsite_marker.is_some() && spec.owner_module.is_none() {
            let annotated_index = annotate_receiver_call_placeholder_owner(
                unique_nodes,
                result_edges,
                edge_keys,
                flags,
                ReceiverPlaceholderAnnotation {
                    line: spec.line,
                    method_col: spec.method_col,
                    method_name: &spec.method_name,
                    owner_name: &spec.owner_name,
                    owner_module: None,
                    extra_callsite_marker,
                    binding_marker,
                },
                receiver_annotation_required_callsite_marker(language_name),
            );
            if annotated_index.is_none() {
                append_manual_receiver_call_placeholder_edge(
                    unique_nodes,
                    result_edges,
                    edge_keys,
                    flags,
                    ManualReceiverCallPlaceholder {
                        source_id,
                        file_id,
                        line: spec.line,
                        method_col: spec.method_col,
                        method_name: &spec.method_name,
                        owner_name: &spec.owner_name,
                        owner_module: None,
                        extra_callsite_marker,
                        binding_marker,
                    },
                    callsite_ordinals,
                );
            }
            continue;
        }
        // Binding-marker specs (PHP foreach elements) join the imported-owner
        // specs on the annotate-or-placeholder route even when their owner is
        // file-local: the in-file owner+method lookup below stays unreachable
        // for them, so a second spec for an already-claimed callsite can never
        // mint a competing resolved edge — the marker lands on the existing
        // edge instead and resolution stays with the resolution pass.
        if binding_marker.is_some() || spec.owner_module.is_some() {
            let owner_module = spec.owner_module.as_deref();
            // A class-anchored spec's callsite lives in a constructor body,
            // where the rule file's self-placeholder is never attributed to a
            // callable and is dropped at post-processing; annotating it would
            // strand the owner markers on a doomed edge. The spec appends its
            // own placeholder (source = the class node) below instead.
            let annotated_index = if spec.class_anchored {
                None
            } else {
                annotate_receiver_call_placeholder_owner(
                    unique_nodes,
                    result_edges,
                    edge_keys,
                    flags,
                    ReceiverPlaceholderAnnotation {
                        line: spec.line,
                        method_col: spec.method_col,
                        method_name: &spec.method_name,
                        owner_name: &spec.owner_name,
                        owner_module,
                        extra_callsite_marker,
                        binding_marker,
                    },
                    receiver_annotation_required_callsite_marker(language_name),
                )
            };
            // Order-independent binding-marker landing: when another spec
            // already annotated or resolved this callsite, the annotate pass
            // skips it, but a binding marker must still reach that edge
            // instead of spawning a competing placeholder.
            if annotated_index.is_none()
                && let Some(marker) = binding_marker
                && append_binding_marker_to_existing_callsite_edge(
                    unique_nodes,
                    result_edges,
                    edge_keys,
                    flags,
                    spec.line,
                    spec.method_col,
                    &spec.method_name,
                    marker,
                )
            {
                continue;
            }
            let should_append_manual = if language_name == "dart" {
                if let Some(index) = annotated_index {
                    if let Some(edge) = result_edges.get(index) {
                        edge_keys.remove(&edge_dedup_key(edge, flags));
                    }
                    result_edges.remove(index);
                }
                true
            } else {
                annotated_index.is_none()
            };
            if should_append_manual {
                append_manual_receiver_call_placeholder_edge(
                    unique_nodes,
                    result_edges,
                    edge_keys,
                    flags,
                    ManualReceiverCallPlaceholder {
                        source_id,
                        file_id,
                        line: spec.line,
                        method_col: spec.method_col,
                        method_name: &spec.method_name,
                        owner_name: &spec.owner_name,
                        owner_module,
                        extra_callsite_marker,
                        binding_marker,
                    },
                    callsite_ordinals,
                );
            }
            continue;
        }

        let javascript_property_alias_provenance_only = language_name == "javascript"
            && !spec.receiver_name.starts_with("this")
            && spec.source_name.rsplit_once('.').is_some_and(|(owner, _)| {
                spec.owner_name
                    .strip_prefix(owner)
                    .is_some_and(|suffix| suffix.starts_with('.') && suffix.len() > 1)
            });
        if javascript_property_alias_provenance_only {
            annotate_receiver_call_placeholder_owner(
                unique_nodes,
                result_edges,
                edge_keys,
                flags,
                ReceiverPlaceholderAnnotation {
                    line: spec.line,
                    method_col: spec.method_col,
                    method_name: &spec.method_name,
                    owner_name: &spec.owner_name,
                    owner_module: None,
                    extra_callsite_marker,
                    binding_marker,
                },
                receiver_annotation_required_callsite_marker(language_name),
            );
            continue;
        }

        let python_local_owner_line = python_local_owner_lines
            .as_ref()
            .and_then(|index| index.unique_line(&spec.owner_name));
        let Some(target_id) = member_targets.target(
            &spec.owner_name,
            &spec.method_name,
            file_id,
            spec.allow_global_fallback,
            python_local_owner_line,
        ) else {
            let should_annotate = match language_name {
                "python" => !languages::python::is_implicit_receiver(&spec.receiver_name),
                "go" | "dart" => true,
                "javascript" => {
                    spec.source_name.contains('.')
                        && (spec.receiver_name == "this" || spec.receiver_name.starts_with("this."))
                }
                // A chained `new X(args).Method()` names its owner verbatim;
                // annotating `receiver-owner:X` without a module hands the
                // callsite to the resolution pass's same-root-namespace arm.
                // Inferred owners keep the fail-closed `false`.
                "csharp" => spec.owner_is_syntactic,
                _ => false,
            };
            if should_annotate && spec.class_anchored {
                // Constructor-body chained calls cannot annotate the rule
                // file's self-placeholder (it is dropped unattributed at
                // post-processing); they carry the owner marker on their own
                // placeholder edge, exactly like the imported-owner route.
                append_manual_receiver_call_placeholder_edge(
                    unique_nodes,
                    result_edges,
                    edge_keys,
                    flags,
                    ManualReceiverCallPlaceholder {
                        source_id,
                        file_id,
                        line: spec.line,
                        method_col: spec.method_col,
                        method_name: &spec.method_name,
                        owner_name: &spec.owner_name,
                        owner_module: None,
                        extra_callsite_marker,
                        binding_marker,
                    },
                    callsite_ordinals,
                );
                continue;
            }
            if should_annotate {
                let annotated_index = annotate_receiver_call_placeholder_owner(
                    unique_nodes,
                    result_edges,
                    edge_keys,
                    flags,
                    ReceiverPlaceholderAnnotation {
                        line: spec.line,
                        method_col: spec.method_col,
                        method_name: &spec.method_name,
                        owner_name: &spec.owner_name,
                        owner_module: spec.owner_module.as_deref(),
                        extra_callsite_marker,
                        binding_marker,
                    },
                    receiver_annotation_required_callsite_marker(language_name),
                );
                if language_name == "dart" {
                    if let Some(index) = annotated_index {
                        if let Some(edge) = result_edges.get(index) {
                            edge_keys.remove(&edge_dedup_key(edge, flags));
                        }
                        result_edges.remove(index);
                    }
                    append_manual_receiver_call_placeholder_edge(
                        unique_nodes,
                        result_edges,
                        edge_keys,
                        flags,
                        ManualReceiverCallPlaceholder {
                            source_id,
                            file_id,
                            line: spec.line,
                            method_col: spec.method_col,
                            method_name: &spec.method_name,
                            owner_name: &spec.owner_name,
                            owner_module: None,
                            extra_callsite_marker,
                            binding_marker,
                        },
                        callsite_ordinals,
                    );
                }
            }
            continue;
        };

        let removed_binding_markers = remove_generic_call_placeholders(
            unique_nodes,
            result_edges,
            edge_keys,
            flags,
            spec.line,
            spec.method_col,
            &spec.method_name,
        );

        let mut edge = Edge {
            id: EdgeId(0),
            source: source_id,
            target: target_id,
            kind: EdgeKind::CALL,
            file_node_id: Some(file_id),
            line: spec.line,
            resolved_target: Some(target_id),
            confidence: Some(1.0),
            certainty: Some(ResolutionCertainty::Certain),
            ..Default::default()
        };
        if !flags.legacy_edge_identity {
            let key = (edge.target, edge.line);
            let next = callsite_ordinals.entry(key).or_insert(0);
            *next = next.saturating_add(1);
            ensure_callsite_identity(&mut edge, Some(*next));
        }
        if let Some(marker) = extra_callsite_marker {
            append_callsite_part(&mut edge, marker);
        }
        // Binding markers landed by earlier specs live on the placeholder this
        // resolution just replaced; carry them over so marker landing stays
        // independent of spec-pass ordering. (Specs carrying their own binding
        // marker never reach this branch — the annotate-or-placeholder route
        // above consumes them.)
        for marker in &removed_binding_markers {
            append_callsite_part(&mut edge, marker);
        }
        if !edge_keys.insert(edge_dedup_key(&edge, flags)) {
            continue;
        }
        edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
        result_edges.push(edge);
    }
}

fn language_type_usage_specs(
    language_name: &str,
    tree: &Tree,
    source: &str,
) -> Vec<ManualTypeUsageSpec> {
    // Only languages with a manual type-usage collector carry one on their
    // registry row (C# today); every other language has none.
    languages::extraction_for_language(language_name)
        .and_then(|extraction| extraction.type_usage_specs)
        .map_or_else(Vec::new, |collect| collect(tree, source))
}

/// Canonical id for an import-resolved type-usage reference node.
///
/// The prefix is on the `preserved_canonical_id` list, so the node keeps this
/// identity through canonicalization and the same-root finalize pass can
/// exclude reference nodes from its declaration candidates. Two references to
/// the same imported type in one file share the id and collapse to one node.
fn type_usage_reference_canonical_id(file_name: &str, qualified_name: &str) -> String {
    format!("{TYPE_USAGE_REFERENCE_CANONICAL_PREFIX}{file_name}:{qualified_name}")
}

/// Canonical id for a PENDING same-root type-usage reference node.
///
/// Encodes the referencing file's namespace and the bare type name so the
/// finalize pass can recover the fact from storage alone: identifiers and
/// namespaces never contain `:`, so parsing from the right is unambiguous
/// even when the file name contains `:`.
fn type_usage_pending_canonical_id(
    file_name: &str,
    referencing_namespace: &str,
    target_name: &str,
) -> String {
    format!(
        "{TYPE_USAGE_PENDING_CANONICAL_PREFIX}{file_name}:{referencing_namespace}:{target_name}"
    )
}

/// Emit TYPE_USAGE edges for the specs a language collector proved against
/// its binding tables (P2a).
///
/// The certainty is stamped `Some(Certain)` AT EMIT — the precedent is
/// `push_type_usage_edge` (structural/common.rs), and the justification is
/// the collector's emit gate: a spec exists only for a type that resolved
/// against the file's visible/imported binding tables, and no TYPE_USAGE
/// resolution job exists to stamp it later.
///
/// Source resolution accepts CLASS anchors for class-anchored specs and the
/// usual FUNCTION|METHOD anchors otherwise. Same-file targets bind to the
/// declaration node by exact name+span; import-resolved targets bind to a
/// reference node minted at the use site (the INHERITANCE rule's parent
/// nodes are the in-file precedent for reference nodes), carrying the
/// import-resolved qualified name.
#[allow(clippy::too_many_arguments)]
fn append_manual_type_usage_edges(
    language_name: &str,
    tree: &Tree,
    source: &str,
    unique_nodes: &mut HashMap<NodeId, Node>,
    file_id: NodeId,
    file_name: &str,
    result_edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
) {
    for spec in language_type_usage_specs(language_name, tree, source) {
        let Some(source_id) =
            node_id_by_name_and_span(unique_nodes, &spec.source_name, spec.source_span, |kind| {
                if spec.class_anchored {
                    // Primary constructors put type anchors on structs too.
                    matches!(kind, NodeKind::CLASS | NodeKind::STRUCT)
                } else {
                    matches!(kind, NodeKind::FUNCTION | NodeKind::METHOD)
                }
            })
        else {
            continue;
        };
        // A pending spec's certainty is NOT yet established: the edge is
        // emitted uncertain against a `type_ref_pending:` reference node, and
        // the post-flush finalize pass either proves it (unique same-root
        // declaration) and stamps `certain`, or deletes it.
        let is_pending = spec.pending_namespace.is_some();
        let target_id = match spec.target_declaration_span {
            Some(declaration_span) => {
                let Some(target_id) = node_id_by_name_and_span(
                    unique_nodes,
                    &spec.target_name,
                    declaration_span,
                    is_type_like_kind,
                ) else {
                    continue;
                };
                target_id
            }
            None => {
                let canonical_id = match spec.pending_namespace.as_deref() {
                    Some(referencing_namespace) => type_usage_pending_canonical_id(
                        file_name,
                        referencing_namespace,
                        &spec.target_name,
                    ),
                    None => {
                        let qualified_name = spec
                            .target_module
                            .as_deref()
                            .unwrap_or(spec.target_name.as_str());
                        type_usage_reference_canonical_id(file_name, qualified_name)
                    }
                };
                let reference_id = NodeId(generate_id(&canonical_id));
                unique_nodes.entry(reference_id).or_insert_with(|| Node {
                    id: reference_id,
                    kind: NodeKind::CLASS,
                    serialized_name: spec.target_name.clone(),
                    qualified_name: Some(
                        spec.target_module
                            .clone()
                            .unwrap_or_else(|| spec.target_name.clone()),
                    ),
                    canonical_id: Some(canonical_id),
                    start_line: Some(spec.reference_span.start_line),
                    start_col: Some(spec.reference_span.start_col),
                    end_line: Some(spec.reference_span.end_line),
                    end_col: Some(spec.reference_span.end_col),
                    ..Default::default()
                });
                reference_id
            }
        };
        if source_id == target_id {
            continue;
        }

        let mut edge = Edge {
            id: EdgeId(0),
            source: source_id,
            target: target_id,
            kind: EdgeKind::TYPE_USAGE,
            file_node_id: Some(file_id),
            line: spec.line,
            certainty: (!is_pending).then_some(ResolutionCertainty::Certain),
            ..Default::default()
        };
        if !edge_keys.insert(edge_dedup_key(&edge, flags)) {
            continue;
        }
        edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
        result_edges.push(edge);
    }
}

/// Post-flush completion of the pending same-root TYPE_USAGE channel (P2a).
///
/// Extraction is per-file and cannot see other files' declarations, so a bare
/// type name that no per-file table resolves is emitted as an UNCERTAIN edge
/// against a `type_ref_pending:` reference node. After every file of the run
/// has flushed, this pass — still the producer, still index-time; there is
/// deliberately no TYPE_USAGE job in the resolution pipeline — checks each
/// fact against the project's type declarations:
///
/// * exactly one declaration with that name under the SAME ROOT NAMESPACE as
///   the referencing file (and distinct from the edge source) → the edge is
///   resolved to it and stamped `certain`;
/// * zero or several candidates, or only a self-candidate → the edge is
///   deleted (fail closed), and pending reference nodes nothing uses any
///   more are removed with their occurrences.
///
/// Reference nodes of either flavour never count as declarations (their
/// canonical prefixes exclude them), and a declaration must be
/// namespace-qualified to have a root at all — global-namespace types never
/// participate. Idempotent: a resolved edge carries
/// `resolved_target_node_id` and is never picked up again; a cancelled run
/// leaves only uncertain pending edges, which can never discharge a
/// certainty-gated check and are re-finalized (or removed with their file)
/// by the next run.
fn finalize_pending_type_usage_edges(storage: &mut Storage) -> Result<()> {
    let conn = storage.get_connection();
    let mut pending = Vec::new();
    {
        let mut statement = conn.prepare(
            "SELECT e.id, e.source_node_id, n.canonical_id
             FROM edge e
             JOIN node n ON n.id = e.target_node_id
             WHERE e.kind = ?1
               AND e.resolved_target_node_id IS NULL
               AND n.canonical_id LIKE ?2",
        )?;
        let rows = statement.query_map(
            rusqlite::params![
                EdgeKind::TYPE_USAGE as i32,
                format!("{TYPE_USAGE_PENDING_CANONICAL_PREFIX}%"),
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        for row in rows {
            pending.push(row?);
        }
    }
    if pending.is_empty() {
        return Ok(());
    }

    // Declaration candidates by bare name (the last segment of the qualified
    // name, so nested types match too).
    let mut declarations_by_name: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    {
        let mut statement = conn.prepare(
            "SELECT id, qualified_name FROM node
             WHERE kind IN (?1, ?2, ?3, ?4)
               AND qualified_name LIKE '%.%'
               AND (canonical_id IS NULL
                    OR (canonical_id NOT LIKE ?5 AND canonical_id NOT LIKE ?6))",
        )?;
        let rows = statement.query_map(
            rusqlite::params![
                NodeKind::CLASS as i32,
                NodeKind::STRUCT as i32,
                NodeKind::INTERFACE as i32,
                NodeKind::ENUM as i32,
                format!("{TYPE_USAGE_REFERENCE_CANONICAL_PREFIX}%"),
                format!("{TYPE_USAGE_PENDING_CANONICAL_PREFIX}%"),
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        )?;
        for row in rows {
            let (id, qualified) = row?;
            let Some(qualified) = qualified else { continue };
            let Some(name) = qualified.rsplit('.').next() else {
                continue;
            };
            declarations_by_name
                .entry(name.to_string())
                .or_default()
                .push((id, qualified.clone()));
        }
    }

    let mut resolutions: Vec<(i64, i64)> = Vec::new();
    let mut removals: Vec<i64> = Vec::new();
    for (edge_id, source_node_id, canonical_id) in pending {
        let Some(suffix) = canonical_id.strip_prefix(TYPE_USAGE_PENDING_CANONICAL_PREFIX) else {
            continue;
        };
        // Suffix is `{file}:{referencing_namespace}:{bare_name}`; identifiers
        // and namespaces never contain `:`, so parse from the right.
        let mut parts = suffix.rsplitn(3, ':');
        let (Some(target_name), Some(referencing_namespace)) = (parts.next(), parts.next()) else {
            removals.push(edge_id);
            continue;
        };
        let Some(referencing_root) = referencing_namespace
            .split('.')
            .next()
            .filter(|root| !root.is_empty())
        else {
            removals.push(edge_id);
            continue;
        };
        let mut candidates = declarations_by_name
            .get(target_name)
            .map(|declarations| {
                declarations
                    .iter()
                    .filter(|(_, qualified)| qualified.split('.').next() == Some(referencing_root))
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        candidates.sort_unstable();
        candidates.dedup();
        match candidates.as_slice() {
            [declaration_id] if *declaration_id != source_node_id => {
                resolutions.push((edge_id, *declaration_id));
            }
            _ => removals.push(edge_id),
        }
    }

    {
        let mut resolve = conn.prepare(
            "UPDATE edge SET resolved_target_node_id = ?2, certainty = 'certain' WHERE id = ?1",
        )?;
        for (edge_id, declaration_id) in &resolutions {
            resolve.execute(rusqlite::params![edge_id, declaration_id])?;
        }
        let mut remove = conn.prepare("DELETE FROM edge WHERE id = ?1")?;
        for edge_id in &removals {
            remove.execute(rusqlite::params![edge_id])?;
        }
    }

    // Pending reference nodes nothing references any more (their edge failed
    // closed) leave with their occurrences.
    let orphan_filter = "canonical_id LIKE ?1
               AND NOT EXISTS (
                   SELECT 1 FROM edge e
                   WHERE e.source_node_id = node.id
                      OR e.target_node_id = node.id
                      OR e.resolved_source_node_id = node.id
                      OR e.resolved_target_node_id = node.id
               )";
    conn.execute(
        &format!(
            "DELETE FROM occurrence WHERE element_id IN
             (SELECT id FROM node WHERE {orphan_filter})"
        ),
        rusqlite::params![format!("{TYPE_USAGE_PENDING_CANONICAL_PREFIX}%")],
    )?;
    conn.execute(
        &format!("DELETE FROM node WHERE {orphan_filter}"),
        rusqlite::params![format!("{TYPE_USAGE_PENDING_CANONICAL_PREFIX}%")],
    )?;
    Ok(())
}

fn annotate_ruby_member_call_placeholders(
    tree: &Tree,
    source: &str,
    nodes: &HashMap<NodeId, Node>,
    edges: &mut [Edge],
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
) {
    walk_tree_nodes(tree.root_node(), &mut |node| {
        let Some((_, method_name)) = languages::ruby::member_call(node, source) else {
            return;
        };
        annotate_call_placeholder_marker(
            nodes,
            edges,
            edge_keys,
            flags,
            CallPlaceholderMarkerAnnotation {
                line: Some(node.start_position().row as u32 + 1),
                method_col: member_call_method_col(node, source, &method_name),
                method_name: &method_name,
                marker: languages::ruby::MEMBER_CALLSITE_MARKER,
            },
        );
    });
}

fn annotate_call_placeholder_marker(
    nodes: &HashMap<NodeId, Node>,
    edges: &mut [Edge],
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
    annotation: CallPlaceholderMarkerAnnotation<'_>,
) -> Option<usize> {
    let mut fallback_index = None;
    let mut exact_index = None;
    for (index, edge) in edges.iter().enumerate() {
        if edge.kind != EdgeKind::CALL
            || edge.line != annotation.line
            || edge.resolved_target.is_some()
            || !nodes
                .get(&edge.target)
                .map(|target| {
                    call_placeholder_matches_method(edge, target, annotation.method_name, None)
                })
                .unwrap_or(false)
        {
            continue;
        }

        fallback_index.get_or_insert(index);
        if annotation.method_col.is_none_or(|col| {
            edge_callsite_col(edge) == Some(col)
                || nodes.get(&edge.target).and_then(|target| target.start_col) == Some(col)
        }) {
            exact_index = Some(index);
            break;
        }
    }

    let index = exact_index.or(fallback_index)?;
    let edge = edges.get_mut(index)?;
    let old_key = edge_dedup_key(edge, flags);
    edge_keys.remove(&old_key);
    append_callsite_marker(edge, annotation.marker);
    edge_keys.insert(edge_dedup_key(edge, flags));
    Some(index)
}

fn annotate_receiver_call_placeholder_owner(
    nodes: &HashMap<NodeId, Node>,
    edges: &mut [Edge],
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
    annotation: ReceiverPlaceholderAnnotation<'_>,
    required_callsite_marker: Option<&str>,
) -> Option<usize> {
    if annotation.owner_name.contains('|') || annotation.owner_name.trim().is_empty() {
        return None;
    }
    let owner_marker = format!("{RECEIVER_OWNER_CALLSITE_PREFIX}{}", annotation.owner_name);
    let module_marker = annotation
        .owner_module
        .filter(|module| !module.contains('|') && !module.trim().is_empty())
        .map(|module| format!("{RECEIVER_MODULE_CALLSITE_PREFIX}{module}"));
    let mut fallback_index = None;
    let mut exact_index = None;
    for (index, edge) in edges.iter().enumerate() {
        if edge.kind != EdgeKind::CALL
            || edge.line != annotation.line
            || edge.resolved_target.is_some()
            || callsite_has_receiver_annotation(edge.callsite_identity.as_deref())
            || !callsite_has_marker(edge.callsite_identity.as_deref(), required_callsite_marker)
            || !nodes
                .get(&edge.target)
                .map(|target| {
                    call_placeholder_matches_method(edge, target, annotation.method_name, None)
                })
                .unwrap_or(false)
        {
            continue;
        }

        fallback_index.get_or_insert(index);
        if annotation.method_col.is_none_or(|col| {
            edge_callsite_col(edge) == Some(col)
                || nodes.get(&edge.target).and_then(|target| target.start_col) == Some(col)
        }) {
            exact_index = Some(index);
            break;
        }
    }

    let index = exact_index.or(fallback_index)?;
    let edge = edges.get_mut(index)?;
    let old_key = edge_dedup_key(edge, flags);
    edge_keys.remove(&old_key);
    append_callsite_part(edge, &owner_marker);
    if let Some(marker) = module_marker.as_deref() {
        append_callsite_part(edge, marker);
    }
    if let Some(marker) = annotation.extra_callsite_marker {
        append_callsite_part(edge, marker);
    }
    if let Some(marker) = annotation.binding_marker {
        append_callsite_part(edge, marker);
    }
    edge_keys.insert(edge_dedup_key(edge, flags));
    Some(index)
}

/// Append a receiver-binding marker to the CALL edge already representing a
/// callsite, whether that edge is a still-unresolved placeholder another spec
/// annotated first or the resolved edge an earlier in-file lookup installed.
///
/// This is the order-independence half of binding-marker landing: the
/// annotate pass deliberately skips annotated and resolved edges, so a spec
/// that lost the annotation race lands its marker here instead of spawning a
/// competing placeholder edge. Returns whether a matching edge was found.
#[allow(clippy::too_many_arguments)]
fn append_binding_marker_to_existing_callsite_edge(
    nodes: &HashMap<NodeId, Node>,
    edges: &mut [Edge],
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
    line: Option<u32>,
    method_col: Option<u32>,
    method_name: &str,
    binding_marker: &str,
) -> bool {
    let mut fallback_index = None;
    let mut exact_index = None;
    for (index, edge) in edges.iter().enumerate() {
        if edge.kind != EdgeKind::CALL || edge.line != line {
            continue;
        }
        let target_matches = nodes
            .get(&edge.target)
            .is_some_and(|target| node_matches_name(target, method_name));
        let resolved_matches = edge
            .resolved_target
            .and_then(|resolved_id| nodes.get(&resolved_id))
            .is_some_and(|resolved| node_matches_name(resolved, method_name));
        if !target_matches && !resolved_matches {
            continue;
        }

        fallback_index.get_or_insert(index);
        if method_col.is_none_or(|col| {
            edge_callsite_col(edge) == Some(col)
                || nodes.get(&edge.target).and_then(|target| target.start_col) == Some(col)
        }) {
            exact_index = Some(index);
            break;
        }
    }

    let Some(index) = exact_index.or(fallback_index) else {
        return false;
    };
    let Some(edge) = edges.get_mut(index) else {
        return false;
    };
    let old_key = edge_dedup_key(edge, flags);
    edge_keys.remove(&old_key);
    append_callsite_part(edge, binding_marker);
    edge_keys.insert(edge_dedup_key(edge, flags));
    true
}

struct ManualReceiverCallPlaceholder<'a> {
    source_id: NodeId,
    file_id: NodeId,
    line: Option<u32>,
    method_col: Option<u32>,
    method_name: &'a str,
    owner_name: &'a str,
    owner_module: Option<&'a str>,
    extra_callsite_marker: Option<&'a str>,
    binding_marker: Option<&'a str>,
}

fn append_manual_receiver_call_placeholder_edge(
    nodes: &HashMap<NodeId, Node>,
    edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
    placeholder: ManualReceiverCallPlaceholder<'_>,
    callsite_ordinals: &mut HashMap<(NodeId, Option<u32>), u32>,
) {
    if placeholder.owner_name.contains('|')
        || placeholder.owner_name.trim().is_empty()
        || placeholder
            .owner_module
            .is_some_and(|module| module.contains('|') || module.trim().is_empty())
    {
        return;
    }
    let Some(target_id) = receiver_call_placeholder_target_id(
        nodes,
        placeholder.method_name,
        placeholder.line,
        placeholder.method_col,
    ) else {
        return;
    };
    let mut edge = Edge {
        id: EdgeId(0),
        source: placeholder.source_id,
        target: target_id,
        kind: EdgeKind::CALL,
        file_node_id: Some(placeholder.file_id),
        line: placeholder.line,
        ..Default::default()
    };
    if !flags.legacy_edge_identity {
        let col = placeholder.method_col.or_else(|| {
            let key = (edge.target, edge.line);
            let next = callsite_ordinals.entry(key).or_insert(0);
            *next = next.saturating_add(1);
            Some(*next)
        });
        ensure_callsite_identity(&mut edge, col);
    }
    append_callsite_part(
        &mut edge,
        &format!("{RECEIVER_OWNER_CALLSITE_PREFIX}{}", placeholder.owner_name),
    );
    if let Some(owner_module) = placeholder.owner_module {
        append_callsite_part(
            &mut edge,
            &format!("{RECEIVER_MODULE_CALLSITE_PREFIX}{owner_module}"),
        );
    }
    if let Some(marker) = placeholder.extra_callsite_marker {
        append_callsite_part(&mut edge, marker);
    }
    if let Some(marker) = placeholder.binding_marker {
        append_callsite_part(&mut edge, marker);
    }
    if !edge_keys.insert(edge_dedup_key(&edge, flags)) {
        return;
    }
    edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
    edges.push(edge);
}

fn receiver_call_placeholder_target_id(
    nodes: &HashMap<NodeId, Node>,
    method_name: &str,
    line: Option<u32>,
    method_col: Option<u32>,
) -> Option<NodeId> {
    let mut matches = nodes
        .values()
        .filter(|node| {
            node.kind == NodeKind::UNKNOWN
                && node_matches_name(node, method_name)
                && line.is_none_or(|line| node.start_line == Some(line))
                && method_col.is_none_or(|col| node.start_col == Some(col))
        })
        .map(|node| node.id)
        .collect::<Vec<_>>();
    matches.sort_unstable();
    matches.dedup();
    if matches.len() == 1 {
        Some(matches[0])
    } else {
        None
    }
}

fn callsite_has_marker(callsite_identity: Option<&str>, required_marker: Option<&str>) -> bool {
    let Some(required_marker) = required_marker else {
        return true;
    };
    callsite_identity
        .is_some_and(|identity| identity.split('|').any(|part| part == required_marker))
}

fn callsite_has_receiver_annotation(callsite_identity: Option<&str>) -> bool {
    callsite_identity.is_some_and(|identity| {
        identity.split('|').any(|part| {
            part.starts_with(RECEIVER_OWNER_CALLSITE_PREFIX)
                || part.starts_with(RECEIVER_MODULE_CALLSITE_PREFIX)
        })
    })
}

#[derive(Clone, Copy)]
struct PreparedMemberOwner {
    id: NodeId,
    file_node_id: Option<NodeId>,
    start_line: Option<u32>,
    span_width: u32,
}

#[derive(Clone, Copy)]
struct PreparedMemberTarget {
    file_node_id: Option<NodeId>,
    id: NodeId,
    start_line: Option<u32>,
}

#[derive(Default)]
struct PreparedMemberTargetIndex {
    owners_by_name: HashMap<String, Vec<PreparedMemberOwner>>,
    targets_by_owner_and_name: HashMap<(NodeId, String), Vec<PreparedMemberTarget>>,
}

impl PreparedMemberTargetIndex {
    fn prepare(nodes: &HashMap<NodeId, Node>, edges: &[Edge]) -> Self {
        let mut index = Self::default();
        for node in nodes.values() {
            count_manual_receiver_lookup_work(1);
            if !is_type_like_kind(node.kind) {
                continue;
            }
            let owner = PreparedMemberOwner {
                id: node.id,
                file_node_id: node.file_node_id,
                start_line: node.start_line,
                span_width: node_span_width(node),
            };
            for name in prepared_node_names(node, true) {
                index.owners_by_name.entry(name).or_default().push(owner);
            }
        }
        for edge in edges {
            count_manual_receiver_lookup_work(1);
            if edge.kind != EdgeKind::MEMBER {
                continue;
            }
            let Some(target) = nodes.get(&edge.target) else {
                continue;
            };
            if !matches!(target.kind, NodeKind::FUNCTION | NodeKind::METHOD) {
                continue;
            }
            let prepared = PreparedMemberTarget {
                file_node_id: target.file_node_id,
                id: target.id,
                start_line: target.start_line,
            };
            for name in prepared_node_names(target, false) {
                index
                    .targets_by_owner_and_name
                    .entry((edge.source, name))
                    .or_default()
                    .push(prepared);
            }
        }
        for owners in index.owners_by_name.values_mut() {
            owners.sort_by(|left, right| {
                left.start_line
                    .unwrap_or(u32::MAX)
                    .cmp(&right.start_line.unwrap_or(u32::MAX))
                    .then_with(|| right.span_width.cmp(&left.span_width))
                    .then_with(|| left.id.cmp(&right.id))
            });
            owners.dedup_by_key(|owner| owner.id);
        }
        for targets in index.targets_by_owner_and_name.values_mut() {
            targets.sort_by_key(|target| (target.start_line.unwrap_or(u32::MAX), target.id));
            targets.dedup_by_key(|target| target.id);
        }
        index
    }

    fn target(
        &self,
        owner_name: &str,
        method_name: &str,
        file_id: NodeId,
        allow_global_fallback: bool,
        owner_start_line: Option<u32>,
    ) -> Option<NodeId> {
        count_manual_receiver_lookup_work(1);
        let owner_lookup_name = owner_start_line
            .and_then(|_| owner_name.rsplit_once('.').map(|(_, name)| name))
            .unwrap_or(owner_name);
        let owners = self
            .owners_by_name
            .get(owner_lookup_name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut candidates = Vec::new();
        for owner in owners {
            count_manual_receiver_lookup_work(1);
            if owner_start_line.is_some_and(|line| owner.start_line != Some(line)) {
                continue;
            }
            if let Some(targets) = self
                .targets_by_owner_and_name
                .get(&(owner.id, method_name.to_owned()))
            {
                for target in targets {
                    count_manual_receiver_lookup_work(1);
                    candidates.push((owner.file_node_id, target.file_node_id, target.id));
                }
            }
        }
        let mut same_file_matches = candidates
            .iter()
            .filter_map(|(owner_file_id, target_file_id, target_id)| {
                (owner_file_id.is_none()
                    || target_file_id.is_none()
                    || *owner_file_id == Some(file_id)
                    || *target_file_id == Some(file_id))
                .then_some(*target_id)
            })
            .collect::<Vec<_>>();
        same_file_matches.sort_unstable();
        same_file_matches.dedup();
        match same_file_matches.as_slice() {
            [target] => return Some(*target),
            [] => {}
            _ => return None,
        }
        if !allow_global_fallback {
            return None;
        }
        let mut global_matches = candidates
            .into_iter()
            .map(|(_, _, target_id)| target_id)
            .collect::<Vec<_>>();
        global_matches.sort_unstable();
        global_matches.dedup();
        match global_matches.as_slice() {
            [target] => Some(*target),
            _ => None,
        }
    }
}

fn prepared_node_names(node: &Node, include_qualified_suffixes: bool) -> Vec<String> {
    let mut names = vec![
        node.serialized_name.clone(),
        short_member_name(&node.serialized_name).to_owned(),
    ];
    if let Some(qualified) = node.qualified_name.as_deref() {
        names.push(qualified.to_owned());
        names.push(short_member_name(qualified).to_owned());
        if include_qualified_suffixes {
            names.extend(
                qualified
                    .match_indices('.')
                    .map(|(index, _)| qualified[index + 1..].to_owned()),
            );
        }
    }
    names.sort();
    names.dedup();
    names
}

#[derive(Default)]
struct PythonLocalOwnerLineIndex {
    lines_by_owner: HashMap<String, Vec<u32>>,
}

impl PythonLocalOwnerLineIndex {
    fn prepare(tree: &Tree, source: &str) -> Self {
        let mut index = Self::default();
        walk_tree_nodes(tree.root_node(), &mut |node| {
            count_manual_receiver_lookup_work(1);
            if node.kind() != "class_definition" {
                return;
            }
            let Some(class_name) = declaration_name(node, source) else {
                return;
            };
            let Some(callable) = enclosing_node_with_kind(node, &["function_definition"]) else {
                return;
            };
            let Some(callable_name) = declaration_name(callable, source) else {
                return;
            };
            index
                .lines_by_owner
                .entry(format!("{callable_name}.{class_name}"))
                .or_default()
                .push(node.start_position().row as u32 + 1);
        });
        for lines in index.lines_by_owner.values_mut() {
            lines.sort_unstable();
            lines.dedup();
        }
        index
    }

    fn unique_line(&self, owner_name: &str) -> Option<u32> {
        count_manual_receiver_lookup_work(1);
        self.lines_by_owner
            .get(owner_name)
            .and_then(|lines| match lines.as_slice() {
                [line] => Some(*line),
                _ => None,
            })
    }
}

fn remove_generic_call_placeholders(
    nodes: &HashMap<NodeId, Node>,
    edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
    line: Option<u32>,
    method_col: Option<u32>,
    method_name: &str,
) -> Vec<String> {
    let mut removed = Vec::new();
    let mut removed_binding_markers = Vec::new();
    edges.retain(|edge| {
        let remove = edge.kind == EdgeKind::CALL
            && edge.line == line
            && edge.resolved_target.is_none()
            && nodes
                .get(&edge.target)
                .map(|target| {
                    call_placeholder_matches_method(edge, target, method_name, method_col)
                })
                .unwrap_or(false);
        if remove {
            removed.push(edge_dedup_key(edge, flags));
            removed_binding_markers.extend(
                edge.callsite_identity
                    .as_deref()
                    .into_iter()
                    .flat_map(|identity| identity.split('|'))
                    .filter(|part| part.starts_with(RECEIVER_BINDING_CALLSITE_PREFIX))
                    .map(str::to_string),
            );
        }
        !remove
    });
    for key in removed {
        edge_keys.remove(&key);
    }
    removed_binding_markers
}

fn call_placeholder_matches_method(
    edge: &Edge,
    target: &Node,
    method_name: &str,
    method_col: Option<u32>,
) -> bool {
    target.kind == NodeKind::UNKNOWN
        && node_matches_name(target, method_name)
        && method_col
            .is_none_or(|col| edge_callsite_col(edge) == Some(col) || target.start_col == Some(col))
}

fn edge_callsite_col(edge: &Edge) -> Option<u32> {
    edge.callsite_identity
        .as_deref()?
        .split('|')
        .next()?
        .split(':')
        .nth(2)?
        .parse()
        .ok()
}

fn descendant_by_field_name<'tree>(node: TsNode<'tree>, field_name: &str) -> Option<TsNode<'tree>> {
    if let Some(child) = node.child_by_field_name(field_name) {
        return Some(child);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = descendant_by_field_name(child, field_name) {
            return Some(found);
        }
    }
    None
}

fn first_descendant_with_kind<'tree>(node: TsNode<'tree>, kind: &str) -> Option<TsNode<'tree>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = first_descendant_with_kind(child, kind) {
            return Some(found);
        }
    }
    None
}

fn node_is_same_or_ancestor(ancestor: TsNode<'_>, node: TsNode<'_>) -> bool {
    let mut current = Some(node);
    while let Some(candidate) = current {
        if same_ts_span(candidate, ancestor) {
            return true;
        }
        current = candidate.parent();
    }
    false
}

fn receiver_callsite_key(spec: &ManualReceiverCallSpec) -> ReceiverCallSiteKey {
    ReceiverCallSiteKey {
        receiver_name: spec.receiver_name.clone(),
        method_name: spec.method_name.clone(),
        line: spec.line,
        method_col: spec.method_col,
    }
}

fn quoted_literal_surface(surface: &str) -> Option<&str> {
    let mut chars = surface.chars();
    let quote = chars.next()?;
    if !matches!(quote, '"' | '\'') {
        return None;
    }
    let end = surface[quote.len_utf8()..].find(quote)? + quote.len_utf8();
    Some(&surface[quote.len_utf8()..end])
}

fn collect_receiver_call_specs_in_callable(
    callable: TsNode<'_>,
    source: &str,
    call_source: ManualReceiverSource<'_>,
    receiver_types: &HashMap<String, String>,
    call_parts: fn(TsNode<'_>, &str) -> Option<(String, String)>,
    allow_global_fallback: bool,
    edges: &mut Vec<ManualReceiverCallSpec>,
) {
    walk_tree_nodes(callable, &mut |node| {
        let Some((receiver_name, method_name)) = call_parts(node, source) else {
            return;
        };
        if !receiver_call_belongs_to_callable(node, callable) {
            return;
        }
        let Some(owner_name) = receiver_types.get(&receiver_name) else {
            return;
        };
        let method_col = member_call_method_col(node, source, &method_name);
        edges.push(ManualReceiverCallSpec {
            source_name: call_source.name.to_string(),
            source_span: call_source.span,
            receiver_name,
            owner_name: owner_name.clone(),
            owner_module: None,
            method_name,
            method_col,
            line: Some(node.start_position().row as u32 + 1),
            allow_global_fallback,
            binding_marker: None,
            required_callsite_marker: None,
            class_anchored: false,
            owner_is_syntactic: false,
        });
    });
}

fn member_call_method_col(node: TsNode<'_>, source: &str, method_name: &str) -> Option<u32> {
    if let Some(col) = languages::python::attribute_method_col(node, source, method_name) {
        return Some(col);
    }

    let text = node_source_text(node, source)?;
    let callable = text.split('(').next().unwrap_or(text.as_str());
    let marker = format!(".{method_name}");
    let method_offset = callable
        .rfind(&marker)
        .map(|offset| offset + 1)
        .or_else(|| callable.rfind(method_name))?;
    Some(node.start_position().column as u32 + method_offset as u32 + 1)
}

fn receiver_call_belongs_to_callable(node: TsNode<'_>, callable: TsNode<'_>) -> bool {
    const DECLARATION_BOUNDARY_KINDS: &[&str] = &[
        "function_definition",
        "function_declaration",
        "method_declaration",
        "method_definition",
        "method",
        "singleton_method",
        "lambda",
        "lambda_expression",
        "arrow_function",
        "function_expression",
        "anonymous_function",
        "closure_expression",
        "class_definition",
        "class_declaration",
        // Constructor bodies are walked by the C# collector with the
        // constructor node as `callable` (P2b). The kind is a *more specific*
        // boundary than the class that always encloses it, so adding it can
        // only change the answer for constructor callables — every existing
        // callable kind saw the same `false` for constructor-body calls
        // before and after (the nearest boundary changes from the class to
        // the constructor, and neither matches a method-like callable).
        "constructor_declaration",
        // C# structs bound scopes exactly like classes (fields, constructors,
        // primary constructors); same neutrality argument — a more specific
        // boundary can only change the answer for struct-shaped callables,
        // and no other grammar names its structs `struct_declaration`.
        "struct_declaration",
    ];
    const BODY_BOUNDARY_KINDS: &[&str] = &["function_body"];

    let boundary_kinds = if callable.kind() == "function_body" {
        BODY_BOUNDARY_KINDS
    } else {
        DECLARATION_BOUNDARY_KINDS
    };

    enclosing_node_with_kind(node, boundary_kinds)
        .is_some_and(|nearest| nearest.kind() == callable.kind() && same_ts_span(nearest, callable))
}

fn collect_colon_parameter_types(callable: TsNode<'_>, source: &str) -> HashMap<String, String> {
    let mut receiver_types = HashMap::new();
    let Some(parameters) = signature_parameter_surface(callable, source) else {
        return receiver_types;
    };
    for parameter in split_top_level_parameters(&parameters) {
        let Some((name_side, type_side)) = parameter.split_once(':') else {
            continue;
        };
        let Some(receiver_name) = parameter_name_before_colon(name_side) else {
            continue;
        };
        let Some(owner_name) = normalize_type_surface(&parameter_type_after_colon(type_side))
        else {
            continue;
        };
        receiver_types.insert(receiver_name, owner_name);
    }
    receiver_types
}

/// Drop the annotations a parameter declaration carries before its type.
///
/// `f(@RequestParam("id") String id)` records the owner type of `id` as
/// `@RequestParam`, because the leading annotation is just another
/// whitespace-separated token and `normalize_type_surface` truncates it at the
/// `(` (CR-009). Token filtering is not enough: `@RequestParam(value = "id")`
/// spans four tokens. Skip each leading `@Name` and, when it is followed by an
/// argument list, everything up to that list's matching `)`.
fn strip_leading_parameter_annotations(parameter: &str) -> &str {
    let mut rest = parameter.trim_start();
    while let Some(after_at) = rest.strip_prefix('@') {
        let name_len = after_at
            .find(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '.'))
            .unwrap_or(after_at.len());
        if name_len == 0 {
            break;
        }
        let after_name = after_at[name_len..].trim_start();
        let Some(arguments) = after_name.strip_prefix('(') else {
            rest = after_name;
            continue;
        };
        let mut depth = 1usize;
        let mut consumed = None;
        for (index, ch) in arguments.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        consumed = Some(index + ch.len_utf8());
                        break;
                    }
                }
                _ => {}
            }
        }
        // An unbalanced annotation argument list is malformed input; leaving
        // the text untouched keeps the caller's existing behaviour rather than
        // inventing a truncation.
        let Some(consumed) = consumed else {
            break;
        };
        rest = arguments[consumed..].trim_start();
    }
    rest
}

fn collect_prefix_parameter_types(callable: TsNode<'_>, source: &str) -> HashMap<String, String> {
    let mut receiver_types = HashMap::new();
    let Some(parameters) = signature_parameter_surface(callable, source) else {
        return receiver_types;
    };
    for parameter in split_top_level_parameters(&parameters) {
        // Annotations come off before the default-value split: an annotation
        // argument list can itself contain `=`, as in `@RequestParam(value =
        // "id")`, and splitting first would cut the declaration in half.
        let parameter = strip_leading_parameter_annotations(&parameter);
        let parameter = parameter.split('=').next().unwrap_or(parameter).trim();
        let tokens = parameter
            .split_whitespace()
            .filter(|token| !matches!(*token, "final" | "const" | "var" | "required"))
            .collect::<Vec<_>>();
        if tokens.len() < 2 {
            continue;
        }
        let receiver_name = tokens.last().copied().unwrap_or_default();
        if receiver_name.starts_with("this.") || receiver_name.starts_with("super.") {
            continue;
        }
        let raw_type = tokens[..tokens.len() - 1].join(" ");
        let Some(receiver_name) = normalize_parameter_name(receiver_name) else {
            continue;
        };
        let Some(owner_name) = normalize_type_surface(&raw_type) else {
            continue;
        };
        receiver_types.insert(receiver_name, owner_name);
    }
    receiver_types
}

/// The grammar's own parameter-list node for a callable, when it has one.
///
/// Every vendored grammar that models parameters exposes the list either
/// through a `parameters` field or as a distinctly named child; only the
/// declarator-nested C family and a few looser grammars leave nothing to find.
fn callable_parameter_list_node<'tree>(callable: TsNode<'tree>) -> Option<TsNode<'tree>> {
    if let Some(parameters) = callable.child_by_field_name("parameters") {
        return Some(parameters);
    }
    let mut cursor = callable.walk();
    callable.named_children(&mut cursor).find(|child| {
        matches!(
            child.kind(),
            "function_value_parameters"
                | "formal_parameters"
                | "parameter_list"
                | "parameters"
                | "class_parameters"
        )
    })
}

/// The text between a callable's parameter parentheses.
///
/// The scan used to start from the first `(` in the callable's own text, which
/// for Java `method_declaration` and Kotlin `function_declaration` includes the
/// leading modifier list: `@GetMapping("/users") String list()` handed back
/// `"/users"` as the parameter list, and `@Throws(IOException::class)` handed
/// back a bogus `IOException::class` receiver binding (CR-009). The grammar
/// already separates modifiers from parameters, so ask it first and keep the
/// text scan only for grammars that expose no parameter node.
fn signature_parameter_surface(callable: TsNode<'_>, source: &str) -> Option<String> {
    if let Some(parameters) = callable_parameter_list_node(callable)
        && let Some(text) = trimmed_node_text(parameters, source)
    {
        return Some(
            text.strip_prefix('(')
                .and_then(|inner| inner.strip_suffix(')'))
                .unwrap_or(text.as_str())
                .to_string(),
        );
    }
    let text = trimmed_node_text(callable, source)?;
    let start = text.find('(')?;
    let mut depth = 0usize;
    let mut parameter_start = None;
    for (index, ch) in text.char_indices().skip_while(|(index, _)| *index < start) {
        match ch {
            '(' => {
                depth = depth.saturating_add(1);
                if depth == 1 {
                    parameter_start = Some(index + ch.len_utf8());
                }
            }
            ')' => {
                if depth == 1 {
                    let parameter_start = parameter_start?;
                    return Some(text[parameter_start..index].to_string());
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_parameters(parameters: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut angle_depth = 0usize;
    for ch in parameters.chars() {
        match ch {
            '(' => paren_depth = paren_depth.saturating_add(1),
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth = bracket_depth.saturating_add(1),
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth = brace_depth.saturating_add(1),
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '<' => angle_depth = angle_depth.saturating_add(1),
            '>' => angle_depth = angle_depth.saturating_sub(1),
            ',' if paren_depth == 0
                && bracket_depth == 0
                && brace_depth == 0
                && angle_depth == 0 =>
            {
                let part = current.trim();
                if !part.is_empty() {
                    parts.push(part.to_string());
                }
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    let part = current.trim();
    if !part.is_empty() {
        parts.push(part.to_string());
    }
    parts
}

fn parameter_name_before_colon(name_side: &str) -> Option<String> {
    name_side
        .split_whitespace()
        .last()
        .and_then(normalize_parameter_name)
}

fn parameter_type_after_colon(type_side: &str) -> String {
    type_side
        .split('=')
        .next()
        .unwrap_or(type_side)
        .split("->")
        .next()
        .unwrap_or(type_side)
        .split("where")
        .next()
        .unwrap_or(type_side)
        .split_whitespace()
        .filter(|token| {
            !matches!(
                *token,
                "inout" | "borrowing" | "consuming" | "some" | "any" | "final" | "const"
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_parameter_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches(',').trim();
    if trimmed == "_" {
        return None;
    }
    let terminal = trimmed.rsplit('.').next().unwrap_or(trimmed);
    let cleaned = terminal
        .trim_start_matches('$')
        .trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '_');
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}

fn normalize_js_ts_private_receiver_surface(receiver: &str) -> String {
    receiver
        .split('.')
        .map(|segment| segment.strip_prefix('#').unwrap_or(segment))
        .collect::<Vec<_>>()
        .join(".")
}

fn normalized_receiver_surface(raw: &str) -> Option<String> {
    let terminal = raw
        .rsplit([' ', '\t', '\n', '\r', '(', '[', '{'])
        .find(|part| !part.trim().is_empty())
        .unwrap_or(raw)
        .trim()
        .trim_end_matches('?')
        .trim();
    normalize_parameter_name(terminal)
}

fn normalized_receiver_variable(node: TsNode<'_>, source: &str) -> Option<String> {
    let text = trimmed_node_text(node, source)?;
    let trimmed = text.trim();
    let without_dollars = trimmed.trim_start_matches('$');
    (!without_dollars.is_empty()).then(|| without_dollars.to_string())
}

fn normalize_type_surface(raw: &str) -> Option<String> {
    let mut surface = raw.trim();
    if surface.contains('|') || surface.contains('&') {
        return None;
    }
    surface = surface.trim_start_matches('?').trim();
    while let Some(stripped) = surface.strip_prefix('*') {
        surface = stripped.trim_start();
    }
    while let Some(stripped) = surface.strip_prefix('&') {
        surface = stripped.trim_start();
    }
    if let Some(stripped) = surface.strip_prefix("[]") {
        surface = stripped.trim_start();
    }
    surface = surface.trim_end_matches('?').trim();
    let base = surface
        .split(['<', '[', '('])
        .next()
        .unwrap_or(surface)
        .trim();
    let terminal = base
        .rsplit(['\\', '.', ':'])
        .find(|segment| !segment.trim().is_empty())
        .unwrap_or(base)
        .trim();
    (!terminal.is_empty()).then(|| terminal.to_string())
}

fn collect_enclosing_type_member_edges(
    tree: &Tree,
    source: &str,
    owner_kinds: &[&str],
    member_kinds: &[&str],
) -> Vec<ManualMemberEdgeSpec> {
    let mut edges = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if !member_kinds.contains(&node.kind()) {
            return;
        }
        let Some(owner_node) = enclosing_node_with_kind(node, owner_kinds) else {
            return;
        };
        let Some(owner_name) = declaration_name(owner_node, source) else {
            return;
        };
        let Some(target_name) = declaration_name(node, source) else {
            return;
        };

        edges.push(ManualMemberEdgeSpec {
            source_name: owner_name,
            target_name,
            source_span: ts_node_graph_span(owner_node),
            target_span: ts_node_graph_span(node),
            line: Some(node.start_position().row as u32 + 1),
        });
    });
    edges
}

fn enclosing_node_with_kind<'tree>(
    mut node: TsNode<'tree>,
    kinds: &[&str],
) -> Option<TsNode<'tree>> {
    while let Some(parent) = node.parent() {
        if kinds.contains(&parent.kind()) {
            return Some(parent);
        }
        node = parent;
    }
    None
}

fn previous_named_sibling_with_kind<'tree>(
    mut node: TsNode<'tree>,
    kinds: &[&str],
) -> Option<TsNode<'tree>> {
    while let Some(previous) = node.prev_named_sibling() {
        if kinds.contains(&previous.kind()) {
            return Some(previous);
        }
        node = previous;
    }
    None
}

fn declaration_name(node: TsNode<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("name")
        .or_else(|| first_named_identifier_like_child(node))
        .and_then(|name_node| trimmed_node_text(name_node, source))
}

fn first_named_identifier_like_child<'tree>(node: TsNode<'tree>) -> Option<TsNode<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).find(|child| {
        matches!(
            child.kind(),
            "identifier"
                | "field_identifier"
                | "type_identifier"
                | "name"
                | "constant"
                | "scope_resolution"
        )
    })
}

fn append_runtime_import_edges(
    specs: &[RuntimeImportSpec],
    unique_nodes: &HashMap<NodeId, Node>,
    file_id: NodeId,
    result_edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
) {
    for spec in specs {
        let source_id = spec
            .binding_node_id
            .filter(|node_id| unique_nodes.contains_key(node_id))
            .unwrap_or(spec.module_node_id);
        let edge = Edge {
            id: EdgeId(generate_edge_id(
                source_id.0,
                spec.module_node_id.0,
                EdgeKind::IMPORT,
            )),
            source: source_id,
            target: spec.module_node_id,
            kind: EdgeKind::IMPORT,
            file_node_id: Some(file_id),
            line: Some(spec.line),
            ..Default::default()
        };
        if !edge_keys.insert(edge_dedup_key(&edge, flags)) {
            continue;
        }
        result_edges.push(edge);
    }
}

fn annotate_exact_runtime_import_bare_calls(
    specs: &[RuntimeImportSpec],
    unique_nodes: &HashMap<NodeId, Node>,
    edges: &mut [Edge],
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
) {
    let exact_target_spans = specs
        .iter()
        .flat_map(|spec| spec.exact_bare_call_target_spans.iter())
        .map(|span| (span.start_line, span.start_col, span.end_line, span.end_col))
        .collect::<HashSet<_>>();
    if exact_target_spans.is_empty() {
        return;
    }

    for edge in edges {
        if edge.kind != EdgeKind::CALL
            || !unique_nodes
                .get(&edge.target)
                .and_then(|target| {
                    Some((
                        target.start_line?,
                        target.start_col?,
                        target.end_line?,
                        target.end_col?,
                    ))
                })
                .is_some_and(|span| exact_target_spans.contains(&span))
        {
            continue;
        }
        edge_keys.remove(&edge_dedup_key(edge, flags));
        append_callsite_marker(edge, languages::javascript::RUNTIME_IMPORT_CALLSITE_MARKER);
        edge.id = EdgeId(generate_edge_id_for_edge(edge, flags));
        edge_keys.insert(edge_dedup_key(edge, flags));
    }
}

fn collect_c_enum_member_pairs(tree: &Tree, source: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    walk_tree_nodes(tree.root_node(), &mut |node| {
        if node.kind() != "enum_specifier" {
            return;
        }
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        let Some(enum_name) = trimmed_node_text(name_node, source) else {
            return;
        };
        let Some(body) = node
            .child_by_field_name("body")
            .or_else(|| first_named_child_with_kind(node, "enumerator_list"))
        else {
            return;
        };

        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if child.kind() != "enumerator" {
                continue;
            }
            if let Some(constant_name_node) = child.child_by_field_name("name")
                && let Some(constant_name) = trimmed_node_text(constant_name_node, source)
            {
                pairs.push((enum_name.clone(), constant_name));
            }
        }
    });
    pairs
}

#[allow(clippy::too_many_arguments)]
fn append_manual_c_enum_member_edges(
    language_name: &str,
    tree: &Tree,
    source: &str,
    unique_nodes: &HashMap<NodeId, Node>,
    file_id: NodeId,
    result_edges: &mut Vec<Edge>,
    edge_keys: &mut HashSet<EdgeDedupKey>,
    flags: IndexFeatureFlags,
) {
    if language_name != "c" && language_name != "cpp" {
        return;
    }

    for (enum_name, constant_name) in collect_c_enum_member_pairs(tree, source) {
        let Some(source_id) =
            unique_node_id_by_name(unique_nodes, &enum_name, |kind| kind == NodeKind::ENUM)
        else {
            continue;
        };
        let Some(target_id) = unique_node_id_by_name(unique_nodes, &constant_name, |kind| {
            kind == NodeKind::ENUM_CONSTANT
        }) else {
            continue;
        };

        let edge = Edge {
            id: EdgeId(generate_edge_id(source_id.0, target_id.0, EdgeKind::MEMBER)),
            source: source_id,
            target: target_id,
            kind: EdgeKind::MEMBER,
            file_node_id: Some(file_id),
            certainty: Some(ResolutionCertainty::Certain),
            ..Default::default()
        };
        if !edge_keys.insert(edge_dedup_key(&edge, flags)) {
            continue;
        }
        result_edges.push(edge);
    }
}

fn suppress_runtime_import_call_edges(
    nodes: &[Node],
    edges: &mut Vec<Edge>,
    runtime_import_specs: &[RuntimeImportSpec],
) {
    if runtime_import_specs.is_empty() {
        return;
    }

    let suppressed = runtime_import_specs
        .iter()
        .map(|spec| {
            (
                spec.suppress_line,
                spec.suppress_start_col,
                spec.suppress_callee_name.as_str(),
            )
        })
        .collect::<HashSet<_>>();
    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();

    edges.retain(|edge| {
        if edge.kind != EdgeKind::CALL {
            return true;
        }
        let Some(line) = edge.line else {
            return true;
        };
        let Some(target_node) = node_by_id.get(&edge.target) else {
            return true;
        };
        let Some(start_col) = edge
            .callsite_identity
            .as_deref()
            .and_then(callsite_identity_start_col)
            .or(target_node.start_col)
        else {
            return true;
        };
        let target_name = short_member_name(&target_node.serialized_name);
        !suppressed.contains(&(line, start_col, target_name))
    });
}

fn callsite_identity_start_col(identity: &str) -> Option<u32> {
    let canonical = identity.split('|').next()?;
    let mut parts = canonical.split(':');
    let _file = parts.next()?;
    let _line = parts.next()?;
    parts.next()?.parse().ok()
}

fn infer_access_from_source(
    language_name: &str,
    tree: &Tree,
    source: &str,
    lines: &LineOffsets,
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

    if let Some(line_text) = lines.line(source, start_line) {
        let access = match language_name {
            "rust" => classify_rust_visibility(line_text),
            _ => classify_keyword_access(line_text),
        };
        if access.is_some() {
            return access;
        }
    }
    if let Some(prev_line) = start_line
        .checked_sub(1)
        .and_then(|line| lines.line(source, line))
    {
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
    canonical_roles: &HashMap<NodeId, CanonicalNodeRole>,
    file_id: NodeId,
) -> Vec<Occurrence> {
    let mut occurrences = Vec::new();
    for node in unique_nodes.values() {
        if let (Some(start_line), Some(start_col), Some(end_line), Some(end_col)) =
            (node.start_line, node.start_col, node.end_line, node.end_col)
        {
            let kind = if matches!(
                canonical_roles.get(&node.id),
                Some(CanonicalNodeRole::Declaration | CanonicalNodeRole::ForwardDeclaration)
            ) {
                codestory_contracts::graph::OccurrenceKind::DECLARATION
            } else {
                codestory_contracts::graph::OccurrenceKind::DEFINITION
            };
            occurrences.push(Occurrence {
                element_id: node.id.0,
                kind,
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
            let qualified_name = node
                .qualified_name
                .clone()
                .unwrap_or_else(|| node.serialized_name.clone());
            queue.push((*id, qualified_name));
        }
    }

    while let Some((parent_id, parent_qualified_name)) = queue.pop() {
        let parent_serialized_name = node_map
            .get(&parent_id)
            .map(|parent_node| parent_node.serialized_name.clone())
            .unwrap_or_else(|| parent_qualified_name.clone());
        let parent_is_type_like = node_map
            .get(&parent_id)
            .is_some_and(|parent_node| is_type_like_kind(parent_node.kind));
        let mut traversal = QualifiedNameTraversal {
            language_name,
            parent_map: &parent_map,
            node_map: &mut node_map,
        };
        queue_qualified_child_names(
            QualifiedNameParent {
                id: parent_id,
                qualified_name: &parent_qualified_name,
                serialized_name: &parent_serialized_name,
                is_type_like: parent_is_type_like,
            },
            &mut traversal,
            &mut queue,
        );
    }

    node_map.into_values().collect()
}

struct QualifiedNameParent<'a> {
    id: NodeId,
    qualified_name: &'a str,
    serialized_name: &'a str,
    is_type_like: bool,
}

struct QualifiedNameTraversal<'a> {
    language_name: &'a str,
    parent_map: &'a HashMap<NodeId, Vec<NodeId>>,
    node_map: &'a mut HashMap<NodeId, Node>,
}

fn queue_qualified_child_names(
    parent: QualifiedNameParent<'_>,
    traversal: &mut QualifiedNameTraversal<'_>,
    queue: &mut Vec<(NodeId, String)>,
) {
    let Some(children) = traversal.parent_map.get(&parent.id) else {
        return;
    };
    for child_id in children {
        let Some(child_node) = traversal.node_map.get_mut(child_id) else {
            continue;
        };
        let delimiter = qualified_name_delimiter(traversal.language_name);
        let new_name = format!(
            "{}{}{}",
            parent.qualified_name, delimiter, child_node.serialized_name
        );
        // Keep members of type-like owners owner-qualified in both name fields so
        // downstream resolution can distinguish declared members from placeholder/reference nodes.
        if parent.is_type_like {
            if promotes_type_member_functions_to_methods(traversal.language_name)
                && child_node.kind == NodeKind::FUNCTION
            {
                child_node.kind = NodeKind::METHOD;
            }
            child_node.serialized_name = format!(
                "{}{}{}",
                parent.serialized_name, delimiter, child_node.serialized_name
            );
        }
        child_node.qualified_name = Some(new_name.clone());
        queue.push((*child_id, new_name));
    }
}

fn promotes_type_member_functions_to_methods(language_name: &str) -> bool {
    // Swift and Dart were the last two languages answering from the roster
    // here; both have rows now, so nothing reaches past the registry.
    languages::extraction_for_language(language_name)
        .is_some_and(|extraction| extraction.promotes_type_member_functions_to_methods)
}

fn qualified_name_delimiter(language_name: &str) -> &'static str {
    // `rust` and `c` were the two `::` languages left in the roster; both
    // carry the delimiter on their rows now, and everything the registry does
    // not know uses `.` exactly as it did before.
    languages::extraction_for_language(language_name)
        .map_or(".", |extraction| extraction.qualified_name_delimiter)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanonicalNodeRole {
    Definition,
    Declaration,
    ForwardDeclaration,
    ImplAnchor,
    Reference,
    Unspecified,
}

fn canonical_role_from_graph_attr(value: &str) -> CanonicalNodeRole {
    match value {
        "declaration" => CanonicalNodeRole::Declaration,
        "forward_declaration" => CanonicalNodeRole::ForwardDeclaration,
        "impl_anchor" => CanonicalNodeRole::ImplAnchor,
        _ => CanonicalNodeRole::Unspecified,
    }
}

fn canonical_role_priority(role: CanonicalNodeRole) -> u8 {
    match role {
        CanonicalNodeRole::Definition => 4,
        CanonicalNodeRole::Declaration => 3,
        CanonicalNodeRole::Unspecified => 2,
        CanonicalNodeRole::ForwardDeclaration => 1,
        CanonicalNodeRole::ImplAnchor | CanonicalNodeRole::Reference => 0,
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
    canonicalize_nodes_with_file_identity(file_name, file_name, final_nodes, canonical_roles)
}

/// Separator between a qualified name and its declaration ordinal.
///
/// A colon would read as the old line-number suffix; `#` makes the ordinal
/// unmistakable in stored ids, logs, and citations.
const DECLARATION_ORDINAL_SEPARATOR: char = '#';

fn node_needs_declaration_ordinal(node: &Node) -> bool {
    !is_type_like_kind(node.kind) && node.kind != NodeKind::FILE
}

fn preserved_canonical_id(node: &Node) -> Option<&str> {
    node.canonical_id.as_deref().filter(|value| {
        value.starts_with("openapi:endpoint:")
            || value.starts_with("route_endpoint:")
            || value.starts_with("tauri:command:")
            || value.starts_with("payload:collection:")
            || value.starts_with(TYPE_USAGE_REFERENCE_CANONICAL_PREFIX)
            || value.starts_with(TYPE_USAGE_PENDING_CANONICAL_PREFIX)
    })
}

/// Canonical ordinals for every qualified name that needs a discriminator.
///
/// Two callables in one file can share a qualified name — Java and C++
/// overloads, an unresolved call placeholder beside the function it names, a
/// local rebound in two sibling scopes. The discriminator used to be the
/// declaration's own `start_line`, which made every identity in the file a
/// function of its position: inserting a line above a function renamed it, so
/// incremental indexing could only replace the whole file (CR-008) and every
/// annotation anchored to it was destroyed (ARCH-001).
///
/// Declarations receive the first source-ordered ordinals for their qualified
/// name. Reference/placeholder nodes are ordered only after that declaration
/// range, so adding or moving a callsite cannot rename a declaration. Columns
/// keep distinct declarations on the same line distinct.
fn canonical_node_ordinals(
    nodes: &[Node],
    canonical_roles: &HashMap<NodeId, CanonicalNodeRole>,
) -> HashMap<NodeId, usize> {
    let mut nodes_by_name: HashMap<String, Vec<&Node>> = HashMap::new();
    for node in nodes {
        if preserved_canonical_id(node).is_some() || !node_needs_declaration_ordinal(node) {
            continue;
        }
        let qualified_name = node
            .qualified_name
            .clone()
            .unwrap_or_else(|| node.serialized_name.clone());
        nodes_by_name.entry(qualified_name).or_default().push(node);
    }
    let mut ordinals = HashMap::new();
    for nodes in nodes_by_name.values_mut() {
        nodes.sort_by_key(|node| {
            (
                node.start_line.unwrap_or(u32::MAX),
                node.start_col.unwrap_or(u32::MAX),
                node.end_line.unwrap_or(u32::MAX),
                node.end_col.unwrap_or(u32::MAX),
                node.kind as i32,
                node.id,
            )
        });
        let (declarations, references): (Vec<_>, Vec<_>) =
            nodes.iter().copied().partition(|node| {
                matches!(
                    canonical_roles.get(&node.id),
                    Some(
                        CanonicalNodeRole::Definition
                            | CanonicalNodeRole::Declaration
                            | CanonicalNodeRole::ForwardDeclaration
                    )
                )
            });
        for (ordinal, node) in declarations.into_iter().chain(references).enumerate() {
            ordinals.insert(node.id, ordinal);
        }
    }
    ordinals
}

fn canonicalize_nodes_with_file_identity(
    file_name: &str,
    file_identity: &str,
    final_nodes: Vec<Node>,
    canonical_roles: &HashMap<NodeId, CanonicalNodeRole>,
) -> (Vec<Node>, HashMap<NodeId, NodeId>) {
    let mut id_remap = HashMap::<NodeId, NodeId>::new();
    let mut grouped_nodes = BTreeMap::<String, Vec<Node>>::new();
    let ordinals_by_node = canonical_node_ordinals(&final_nodes, canonical_roles);

    for mut node in final_nodes {
        let qualified_name = node
            .qualified_name
            .clone()
            .unwrap_or_else(|| node.serialized_name.clone());
        node.qualified_name = Some(qualified_name.clone());

        let canonical_id = preserved_canonical_id(&node)
            .map(str::to_string)
            .unwrap_or_else(|| {
                if is_type_like_kind(node.kind) {
                    format!("{}:{}", file_name, qualified_name)
                } else if node.kind == NodeKind::FILE {
                    format!("{file_identity}:{file_identity}:1")
                } else {
                    let ordinal = ordinals_by_node.get(&node.id).copied().unwrap_or(0);
                    format!("{file_name}:{qualified_name}{DECLARATION_ORDINAL_SEPARATOR}{ordinal}")
                }
            });
        grouped_nodes.entry(canonical_id).or_default().push(node);
    }

    let mut deduped_nodes = Vec::with_capacity(grouped_nodes.len());
    for (canonical_id, nodes) in grouped_nodes {
        let new_id = NodeId(generate_id(&canonical_id));
        for node in &nodes {
            id_remap.insert(node.id, new_id);
        }

        // The canonical role selects the authoritative source anchor, while a
        // type reference can still carry the more specific semantic kind.
        let semantic_type_kind = nodes
            .iter()
            .filter(|node| is_type_like_kind(node.kind))
            .max_by_key(|node| type_anchor_priority(node.kind))
            .map(|node| node.kind);

        let mut node = nodes
            .into_iter()
            .max_by(|left, right| compare_canonical_node_candidates(left, right, canonical_roles))
            .unwrap_or_default();
        if let Some(kind) = semantic_type_kind {
            node.kind = kind;
        }
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
        if let Some(resolved_source) = edge.resolved_source
            && let Some(new_id) = id_remap.get(&resolved_source)
        {
            edge.resolved_source = Some(*new_id);
        }
        if let Some(resolved_target) = edge.resolved_target
            && let Some(new_id) = id_remap.get(&resolved_target)
        {
            edge.resolved_target = Some(*new_id);
        }
        for candidate in &mut edge.candidate_targets {
            if let Some(new_id) = id_remap.get(candidate) {
                *candidate = *new_id;
            }
        }
        edge.file_node_id = Some(new_file_id);
        if !flags.legacy_edge_identity {
            refresh_callsite_identity(edge);
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

fn remap_local_node_id(
    edges: &mut [Edge],
    occurrences: &mut [Occurrence],
    from: NodeId,
    to: NodeId,
) {
    for edge in edges {
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

    for occurrence in occurrences {
        if occurrence.element_id == from.0 {
            occurrence.element_id = to.0;
        }
    }
}

fn reconcile_local_rust_impl_anchors(
    nodes: &mut Vec<Node>,
    edges: &mut [Edge],
    occurrences: &mut [Occurrence],
    canonical_roles: &HashMap<NodeId, CanonicalNodeRole>,
) {
    let impl_anchor_ids = nodes
        .iter()
        .filter_map(|node| {
            (canonical_roles.get(&node.id) == Some(&CanonicalNodeRole::ImplAnchor))
                .then_some(node.id)
        })
        .collect::<HashSet<_>>();
    if impl_anchor_ids.is_empty() {
        return;
    }

    let anchors = nodes
        .iter()
        .filter(|node| impl_anchor_ids.contains(&node.id))
        .cloned()
        .collect::<Vec<_>>();
    let mut remaps = Vec::new();
    for anchor in anchors {
        if let Some(target_id) = choose_pending_impl_anchor_target(&anchor, nodes, &impl_anchor_ids)
        {
            remaps.push((anchor.id, target_id));
        }
    }
    if remaps.is_empty() {
        return;
    }

    for (from, to) in &remaps {
        remap_local_node_id(edges, occurrences, *from, *to);
    }

    let removed_ids = remaps.iter().map(|(from, _)| *from).collect::<HashSet<_>>();
    nodes.retain(|node| !removed_ids.contains(&node.id));
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

    for edge in edges
        .iter_mut()
        .filter(|edge| edge.kind == EdgeKind::OVERRIDE)
    {
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

fn canonical_declaration_ordinal(node: &Node) -> Option<usize> {
    node.canonical_id
        .as_deref()?
        .rsplit_once(DECLARATION_ORDINAL_SEPARATOR)?
        .1
        .parse()
        .ok()
}

fn should_replace_reference_candidate(candidate: &Node, current: &Node) -> bool {
    canonical_declaration_ordinal(candidate)
        .cmp(&canonical_declaration_ordinal(current))
        .then_with(|| {
            candidate
                .start_line
                .unwrap_or(u32::MAX)
                .cmp(&current.start_line.unwrap_or(u32::MAX))
                .then_with(|| node_span_width(current).cmp(&node_span_width(candidate)))
        })
        .is_lt()
}

fn reconcile_tsx_usage_targets(nodes: &[Node], edges: &mut [Edge]) {
    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    let mut earliest_by_key = HashMap::<(NodeKind, String), NodeId>::new();
    let mut declaration_by_key = HashMap::<(NodeKind, String), NodeId>::new();
    for node in nodes {
        let key = (
            node.kind,
            short_member_name(&node.serialized_name).to_string(),
        );
        let replace_earliest = earliest_by_key
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
        if replace_earliest {
            earliest_by_key.insert(key.clone(), node.id);
        }
        let replace_declaration = declaration_by_key
            .get(&key)
            .and_then(|current_id| node_by_id.get(current_id))
            .map(|current| should_replace_reference_candidate(node, current))
            .unwrap_or(true);
        if replace_declaration {
            declaration_by_key.insert(key, node.id);
        }
    }

    for edge in edges
        .iter_mut()
        .filter(|edge| matches!(edge.kind, EdgeKind::USAGE | EdgeKind::CALL))
    {
        let Some(target_node) = node_by_id.get(&edge.target).copied() else {
            continue;
        };
        let key = (
            target_node.kind,
            short_member_name(&target_node.serialized_name).to_string(),
        );
        let candidates = if edge.kind == EdgeKind::USAGE {
            &declaration_by_key
        } else {
            &earliest_by_key
        };
        let Some(candidate_id) = candidates.get(&key).copied() else {
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

    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    let mut best_by_key = HashMap::<(NodeKind, String), NodeId>::new();
    for node in nodes.iter() {
        if !matches!(node.kind, NodeKind::FUNCTION | NodeKind::FIELD) {
            continue;
        }
        let key = (
            node.kind,
            short_member_name(&node.serialized_name).to_string(),
        );
        let should_replace = best_by_key
            .get(&key)
            .and_then(|current_id| node_by_id.get(current_id))
            .map(|current| should_replace_reference_candidate(node, current))
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
            let key = (
                node.kind,
                short_member_name(&node.serialized_name).to_string(),
            );
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

#[allow(clippy::too_many_arguments)]
fn post_process_index_results(
    result_nodes: Vec<Node>,
    result_edges: &mut Vec<Edge>,
    result_occurrences: &mut Vec<Occurrence>,
    file_name: &str,
    file_id: NodeId,
    language_name: &str,
    canonical_role_by_node_id: &HashMap<NodeId, CanonicalNodeRole>,
    is_tsx_file: bool,
    runtime_import_specs: &[RuntimeImportSpec],
    flags: IndexFeatureFlags,
) -> PostProcessedIndexResults {
    // Stage 1: qualify names before deduplication so canonical IDs are stable.
    let mut final_nodes = apply_qualified_names(result_nodes, result_edges, language_name);
    if language_name == "rust" {
        reconcile_local_rust_impl_anchors(
            &mut final_nodes,
            result_edges,
            result_occurrences,
            canonical_role_by_node_id,
        );
    }
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
    // Stage 7: runtime module imports should not retain generic CALL placeholders.
    suppress_runtime_import_call_edges(&final_nodes, result_edges, runtime_import_specs);

    PostProcessedIndexResults {
        nodes: final_nodes,
        id_remap,
    }
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
    let candidates = nodes
        .iter()
        .filter(|candidate| {
            candidate.id != anchor.id
                && is_type_like_kind(candidate.kind)
                && !impl_anchor_ids.contains(&candidate.id)
                && (candidate.serialized_name == anchor.serialized_name
                    || short_member_name(&candidate.serialized_name) == anchor.serialized_name)
        })
        .collect::<Vec<_>>();

    if let Some(anchor_qualified_name) = anchor.qualified_name.as_deref() {
        let mut qualified_matches = candidates
            .iter()
            .filter(|candidate| candidate.qualified_name.as_deref() == Some(anchor_qualified_name))
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>();
        qualified_matches.sort_unstable();
        qualified_matches.dedup();
        if let Some(anchor_file_id) = anchor.file_node_id {
            let same_file = qualified_matches
                .iter()
                .copied()
                .filter(|candidate_id| {
                    candidates
                        .iter()
                        .find(|candidate| candidate.id == *candidate_id)
                        .is_some_and(|candidate| candidate.file_node_id == Some(anchor_file_id))
                })
                .collect::<Vec<_>>();
            if same_file.len() == 1 {
                return Some(same_file[0]);
            }
        }
        if qualified_matches.len() == 1 {
            return Some(qualified_matches[0]);
        }
    }

    if let Some(anchor_file_id) = anchor.file_node_id {
        let mut same_file_matches = candidates
            .iter()
            .filter(|candidate| candidate.file_node_id == Some(anchor_file_id))
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>();
        same_file_matches.sort_unstable();
        same_file_matches.dedup();
        if same_file_matches.len() == 1 {
            return Some(same_file_matches[0]);
        }
    }

    let mut matches = candidates
        .iter()
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

fn choose_existing_impl_anchor_target(storage: &Storage, anchor: &Node) -> Result<Option<NodeId>> {
    let mut query = String::from(
        "SELECT id, serialized_name, qualified_name, file_node_id
         FROM node
         WHERE (serialized_name = ?1 OR serialized_name LIKE ?2)
            ",
    );
    query.push_str(non_impl_anchor_canonical_predicate());
    query.push_str(
        "
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
    let mut params = vec![
        rusqlite::types::Value::from(anchor.serialized_name.clone()),
        rusqlite::types::Value::from(format!("%::{}", anchor.serialized_name)),
    ];
    params.extend(
        kind_values
            .iter()
            .copied()
            .map(rusqlite::types::Value::from),
    );
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params), |row| {
            Ok((
                NodeId(row.get::<_, i64>(0)?),
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?.map(NodeId),
            ))
        })
        .map_err(|e| anyhow!("Storage query error: {:?}", e))?;

    let anchor_qualified_name = anchor.qualified_name.as_deref();
    let anchor_file_id = anchor.file_node_id;
    let mut qualified_matches = Vec::new();
    let mut same_file_matches = Vec::new();
    let mut matches = Vec::new();
    for row in rows {
        let (node_id, serialized_name, qualified_name, file_node_id) =
            row.map_err(|e| anyhow!("Storage row error: {:?}", e))?;
        if serialized_name != anchor.serialized_name
            && short_member_name(&serialized_name) != anchor.serialized_name
        {
            continue;
        }
        if qualified_name.as_deref() == anchor_qualified_name {
            qualified_matches.push(node_id);
        }
        if anchor_file_id.is_some() && file_node_id == anchor_file_id {
            same_file_matches.push(node_id);
        }
        matches.push(node_id);
    }

    qualified_matches.sort_unstable();
    qualified_matches.dedup();
    if anchor_file_id.is_some() {
        let qualified_same_file = qualified_matches
            .iter()
            .copied()
            .filter(|node_id| same_file_matches.contains(node_id))
            .collect::<Vec<_>>();
        if qualified_same_file.len() == 1 {
            return Ok(Some(qualified_same_file[0]));
        }
    }
    if qualified_matches.len() == 1 {
        return Ok(Some(qualified_matches[0]));
    }

    same_file_matches.sort_unstable();
    same_file_matches.dedup();
    if same_file_matches.len() == 1 {
        return Ok(Some(same_file_matches[0]));
    }

    matches.sort_unstable();
    matches.dedup();
    Ok(if matches.len() == 1 {
        Some(matches[0])
    } else {
        None
    })
}

fn reconcile_rust_impl_anchors(storage: &Storage, pending: &mut IntermediateStorage) -> Result<()> {
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
        let Some(anchor) = pending
            .nodes
            .iter()
            .find(|node| node.id == anchor_id)
            .cloned()
        else {
            continue;
        };
        if !is_type_like_kind(anchor.kind) {
            continue;
        }

        let target = choose_pending_impl_anchor_target(&anchor, &pending.nodes, &impl_anchor_ids)
            .or_else(|| {
                choose_existing_impl_anchor_target(storage, &anchor)
                    .ok()
                    .flatten()
            });
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
    pending
        .impl_anchor_node_ids
        .retain(|node_id| !removed_ids.contains(node_id));
    pending.impl_anchor_node_ids.sort_unstable();
    pending.impl_anchor_node_ids.dedup();

    Ok(())
}

fn reconcile_local_impl_anchor_nodes(
    nodes: &mut Vec<Node>,
    edges: &mut [Edge],
    occurrences: &mut [Occurrence],
    component_access: &mut [(NodeId, AccessKind)],
    impl_anchor_node_ids: &mut Vec<NodeId>,
) {
    if impl_anchor_node_ids.is_empty() {
        return;
    }

    let impl_anchor_ids = impl_anchor_node_ids.iter().copied().collect::<HashSet<_>>();
    let anchor_ids = impl_anchor_node_ids.clone();
    let mut remaps = Vec::<(NodeId, NodeId)>::new();

    for anchor_id in anchor_ids {
        let Some(anchor) = nodes.iter().find(|node| node.id == anchor_id).cloned() else {
            continue;
        };
        if !is_type_like_kind(anchor.kind) {
            continue;
        }

        if let Some(target_id) = choose_pending_impl_anchor_target(&anchor, nodes, &impl_anchor_ids)
        {
            remaps.push((anchor.id, target_id));
        }
    }

    if remaps.is_empty() {
        return;
    }

    for (from, to) in &remaps {
        for edge in edges.iter_mut() {
            if edge.source == *from {
                edge.source = *to;
            }
            if edge.target == *from {
                edge.target = *to;
            }
            if edge.resolved_source == Some(*from) {
                edge.resolved_source = Some(*to);
            }
            if edge.resolved_target == Some(*from) {
                edge.resolved_target = Some(*to);
            }
        }

        for occurrence in occurrences.iter_mut() {
            if occurrence.element_id == from.0 {
                occurrence.element_id = to.0;
            }
        }

        for (node_id, _) in component_access.iter_mut() {
            if *node_id == *from {
                *node_id = *to;
            }
        }

        for node_id in impl_anchor_node_ids.iter_mut() {
            if *node_id == *from {
                *node_id = *to;
            }
        }
    }

    let removed_ids = remaps.iter().map(|(from, _)| *from).collect::<HashSet<_>>();
    nodes.retain(|node| !removed_ids.contains(&node.id));
    impl_anchor_node_ids.retain(|node_id| !removed_ids.contains(node_id));
    impl_anchor_node_ids.sort_unstable();
    impl_anchor_node_ids.dedup();
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenApiEndpoint {
    method: String,
    path: String,
    line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrameworkRoute {
    framework: &'static str,
    method: String,
    path: String,
    raw_path: String,
    handler: Option<String>,
    line: u32,
    confidence: &'static str,
    source_convention: &'static str,
    extraction_provenance: &'static str,
    claim_tier: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TauriCommandRegistration {
    command: String,
    line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TauriCommandInvocation {
    command: String,
    line: u32,
    col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PayloadCollectionRegistration {
    slug: String,
    line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PayloadCollectionUsage {
    slug: String,
    operation: String,
    line: u32,
    col: u32,
}

impl FrameworkRoute {
    fn new(
        framework: &'static str,
        method: String,
        raw_path: String,
        handler: Option<String>,
        line: u32,
        confidence: &'static str,
    ) -> Self {
        Self {
            framework,
            method,
            path: normalize_framework_route_path(&raw_path),
            raw_path,
            handler,
            line,
            confidence,
            source_convention: confidence,
            extraction_provenance: "line_scan",
            claim_tier: "structural",
        }
    }

    fn with_extraction_provenance(mut self, extraction_provenance: &'static str) -> Self {
        self.extraction_provenance = extraction_provenance;
        self
    }

    fn with_claim_evidence(
        mut self,
        extraction_provenance: &'static str,
        claim_tier: &'static str,
    ) -> Self {
        self.extraction_provenance = extraction_provenance;
        self.claim_tier = claim_tier;
        self
    }

    fn with_confidence(mut self, confidence: &'static str) -> Self {
        self.confidence = confidence;
        self.source_convention = confidence;
        self
    }
}

/// Return whether a path is eligible for OpenAPI schema diagnostics.
///
/// Eligibility is extension-only; `looks_like_openapi_schema` must still verify
/// content before endpoint source proof is emitted.
pub fn is_openapi_candidate_path(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(extension.as_str(), "json" | "yaml" | "yml")
}

fn openapi_path_language_hint(path: &Path) -> bool {
    if !is_openapi_candidate_path(path) {
        return false;
    }
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|stem| stem.contains("openapi") || stem.contains("swagger"))
}

/// Return whether a path can receive text-only framework diagnostics.
///
/// Text-only candidates can contribute source proof for framework routes or
/// endpoint literals, but they are not parser-backed graph coverage.
pub fn is_text_only_candidate_path(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(extension.as_str(), "go" | "rb" | "php" | "cs" | "cshtml")
}

fn text_only_language_name(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    // Component dialects come from the companion-extension registry.
    if let Some(surface) =
        codestory_contracts::language_support::companion_surface_language(&extension)
    {
        return surface;
    }
    match extension.as_str() {
        "go" => "go",
        "rb" => "ruby",
        "php" => "php",
        "cs" | "cshtml" => "csharp",
        _ => "text",
    }
}

/// Return the source-group identity for a companion extension that discovery
/// admits but no parser or structural collector owns.
///
/// This identity is inventory metadata only. It must not be added to the
/// public language-support registry because doing so would advertise a graph
/// or source-proof claim that the indexer cannot make.
fn companion_inventory_language(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?;
    let profile = codestory_contracts::language_support::companion_extension_profile(extension)?;
    profile
        .surface_language
        .or_else(|| profile.source_group_languages.first().copied())
}

/// Persist exact source identity for a discovered companion file without
/// emitting non-file graph evidence.
///
/// `complete = false` keeps proof and absence reasoning conservative. The
/// verified content hash makes an unchanged file stable across incremental
/// refreshes, while an actual byte change still schedules the file again.
fn index_inventory_only_file(path: &Path, language: &str) -> Result<IntermediateStorage> {
    let bytes = std::fs::read(path)?;
    let content_hash = source_content_hash(&bytes);
    let source = String::from_utf8_lossy(&bytes);
    let (file_node, _file_name, file_id) = file_node_from_source(path, &source);
    let mut local_storage = IntermediateStorage::default();
    local_storage.files.push(codestory_store::FileInfo {
        id: file_id.0,
        path: path.to_path_buf(),
        language: language.to_string(),
        modification_time: file_modification_time(path),
        indexed: true,
        complete: false,
        line_count: source.lines().count() as u32,
        file_role: codestory_store::FileRole::classify_path(path),
    });
    local_storage.nodes.push(file_node);
    local_storage
        .file_content_hashes
        .push(codestory_store::FileContentHash {
            file_id: file_id.0,
            content_hash,
        });
    Ok(local_storage)
}

fn prepare_template_index_work(
    path: &Path,
    template_kind: template_pipeline::TemplateKind,
) -> Result<IntermediateStorage> {
    let source = std::fs::read_to_string(path)?;
    index_template_file(path, template_kind, &source)
}

fn parser_source_is_complete(source: &str, language_config: &LanguageConfig) -> Result<bool> {
    let mut parser = Parser::new();
    parser
        .set_language(&language_config.language)
        .map_err(|error| anyhow!("Language error: {error:?}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse template script regions"))?;
    Ok(!tree.root_node().has_error())
}

fn index_template_file(
    path: &Path,
    template_kind: template_pipeline::TemplateKind,
    source: &str,
) -> Result<IntermediateStorage> {
    let content_hash = source_content_hash(source.as_bytes());
    let prepared = template_pipeline::prepare_template_source(template_kind, source);
    let script_ext = match prepared.script_language {
        "typescript" => "ts",
        _ => "js",
    };
    let language_config = get_language_for_ext(script_ext)
        .ok_or_else(|| anyhow!("missing tree-sitter config for template script language"))?;

    let mut index_result = index_file(path, &prepared.blanked, &language_config, None, None)?;
    let parser_region_complete =
        parser_source_is_complete(&prepared.completeness_blanked, &language_config)?;
    let surface_language = template_pipeline::template_surface_language(path).unwrap_or("template");
    if let Some(file_info) = index_result.files.first_mut() {
        file_info.language = surface_language.to_string();
        file_info.complete = parser_region_complete;
    }

    let file_id = index_result
        .files
        .first()
        .map(|file| NodeId(file.id))
        .unwrap_or_else(|| file_node_from_source(path, source).2);

    let mut local_storage = IntermediateStorage::default();
    local_storage.files.extend(index_result.files);
    local_storage.nodes.extend(index_result.nodes);
    local_storage.occurrences.extend(index_result.occurrences);
    local_storage
        .component_access
        .extend(index_result.component_access);
    local_storage
        .impl_anchor_node_ids
        .extend(index_result.impl_anchor_node_ids);
    append_text_only_framework_routes(path, surface_language, source, file_id, &mut local_storage);
    // Insert Tauri invoke edges before tree-sitter CALL edges so SQLite ON CONFLICT keeps
    // the heuristic uncertain boundary evidence when identities collide.
    append_text_only_tauri_invocations(surface_language, source, file_id, &mut local_storage);
    local_storage.edges.extend(index_result.edges);
    template_pipeline::delegate_template_style_blocks(
        path,
        &prepared.style_blocks,
        file_id,
        &mut local_storage,
    );
    let file_identity = WorkspaceIndexer::file_identity_path(path);
    let flags = index_feature_flags();
    let (final_nodes, id_remap) = canonicalize_nodes_with_file_identity(
        &file_identity,
        &file_identity,
        local_storage.nodes,
        &HashMap::new(),
    );
    let new_file_id = id_remap.get(&file_id).copied().unwrap_or(file_id);
    local_storage.nodes = final_nodes;
    let final_node_ids = local_storage
        .nodes
        .iter()
        .map(|node| node.id)
        .collect::<HashSet<_>>();
    remap_file_affinity(&mut local_storage.nodes, new_file_id);
    remap_edges(&mut local_storage.edges, new_file_id, &id_remap, flags);
    remap_occurrences(&mut local_storage.occurrences, &id_remap);
    local_storage.component_access = local_storage
        .component_access
        .into_iter()
        .filter_map(|(node_id, access)| {
            let remapped = id_remap.get(&node_id).copied().unwrap_or(node_id);
            final_node_ids
                .contains(&remapped)
                .then_some((remapped, access))
        })
        .collect();
    local_storage.structural_unit_node_ids = local_storage
        .structural_unit_node_ids
        .into_iter()
        .map(|node_id| id_remap.get(&node_id).copied().unwrap_or(node_id))
        .filter(|node_id| final_node_ids.contains(node_id))
        .collect();
    local_storage.structural_unit_node_ids.sort_unstable();
    local_storage.structural_unit_node_ids.dedup();
    local_storage.impl_anchor_node_ids = local_storage
        .impl_anchor_node_ids
        .into_iter()
        .map(|node_id| id_remap.get(&node_id).copied().unwrap_or(node_id))
        .filter(|node_id| final_node_ids.contains(node_id))
        .collect();
    local_storage.impl_anchor_node_ids.sort_unstable();
    local_storage.impl_anchor_node_ids.dedup();
    if let Some(file_info) = local_storage.files.first_mut()
        && let Some(remapped) = id_remap.get(&NodeId(file_info.id))
    {
        file_info.id = remapped.0;
    }
    if let Some(file_info) = local_storage.files.first() {
        local_storage
            .file_content_hashes
            .push(codestory_store::FileContentHash {
                file_id: file_info.id,
                content_hash,
            });
    }
    local_storage.callable_projection_states = build_callable_projection_states(
        &local_storage.nodes,
        &local_storage.edges,
        &local_storage.occurrences,
    );
    Ok(local_storage)
}

fn index_text_only_file(path: &Path) -> Result<IntermediateStorage> {
    let source = std::fs::read_to_string(path)?;
    let content_hash = source_content_hash(source.as_bytes());
    let mut local_storage = IntermediateStorage::default();
    let (file_node, _file_name, file_id) = file_node_from_source(path, &source);
    local_storage.files.push(codestory_store::FileInfo {
        id: file_id.0,
        path: path.to_path_buf(),
        language: text_only_language_name(path).to_string(),
        modification_time: file_modification_time(path),
        indexed: true,
        complete: true,
        line_count: source.lines().count() as u32,
        file_role: codestory_store::FileRole::classify_path(path),
    });
    local_storage.nodes.push(file_node);
    if text_only_language_name(path) == "go" {
        append_text_only_go_symbols(path, &source, file_id, &mut local_storage);
    }
    append_text_only_framework_routes(
        path,
        text_only_language_name(path),
        &source,
        file_id,
        &mut local_storage,
    );
    append_text_only_tauri_invocations(
        text_only_language_name(path),
        &source,
        file_id,
        &mut local_storage,
    );
    local_storage.callable_projection_states = build_callable_projection_states(
        &local_storage.nodes,
        &local_storage.edges,
        &local_storage.occurrences,
    );
    local_storage
        .file_content_hashes
        .push(codestory_store::FileContentHash {
            file_id: local_storage.files[0].id,
            content_hash,
        });
    Ok(local_storage)
}

#[derive(Debug, Clone)]
struct TextOnlySymbol {
    name: String,
    kind: NodeKind,
    line: u32,
    col: u32,
}

fn append_text_only_go_symbols(
    path: &Path,
    source: &str,
    file_id: NodeId,
    local_storage: &mut IntermediateStorage,
) {
    for symbol in collect_go_text_symbols(source) {
        let node_id = text_only_symbol_node_id(path, &symbol);
        local_storage.nodes.push(Node {
            id: node_id,
            kind: symbol.kind,
            serialized_name: symbol.name.clone(),
            qualified_name: Some(symbol.name.clone()),
            canonical_id: Some(format!(
                "go:symbol:{}:{}:{}",
                path.to_string_lossy(),
                symbol.name,
                symbol.line
            )),
            file_node_id: Some(file_id),
            start_line: Some(symbol.line),
            start_col: Some(symbol.col),
            end_line: Some(symbol.line),
            end_col: Some(symbol.col + symbol.name.len().max(1) as u32),
        });
        local_storage
            .component_access
            .push((node_id, go_symbol_access(&symbol.name)));
        local_storage.edges.push(Edge {
            id: EdgeId(generate_edge_id(file_id.0, node_id.0, EdgeKind::MEMBER)),
            source: file_id,
            target: node_id,
            kind: EdgeKind::MEMBER,
            file_node_id: Some(file_id),
            line: Some(symbol.line),
            certainty: Some(ResolutionCertainty::Certain),
            ..Default::default()
        });
        local_storage.occurrences.push(Occurrence {
            element_id: node_id.0,
            kind: OccurrenceKind::DEFINITION,
            location: SourceLocation {
                file_node_id: file_id,
                start_line: symbol.line,
                start_col: symbol.col,
                end_line: symbol.line,
                end_col: symbol.col + symbol.name.len().max(1) as u32,
            },
        });
    }
}

fn collect_go_text_symbols(source: &str) -> Vec<TextOnlySymbol> {
    let mut symbols = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let line_number = index as u32 + 1;
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        if let Some((name, kind)) = parse_go_func_symbol(trimmed) {
            let terminal_name = name.rsplit('.').next().unwrap_or(name.as_str());
            let col = line
                .find(terminal_name)
                .map(|value| value as u32 + 1)
                .unwrap_or(1);
            symbols.push(TextOnlySymbol {
                name,
                kind,
                line: line_number,
                col,
            });
            continue;
        }
        if let Some((name, kind)) = parse_go_type_symbol(trimmed) {
            let col = line.find(&name).map(|value| value as u32 + 1).unwrap_or(1);
            symbols.push(TextOnlySymbol {
                name,
                kind,
                line: line_number,
                col,
            });
        }
    }
    symbols
}

fn parse_go_func_symbol(line: &str) -> Option<(String, NodeKind)> {
    let rest = line.strip_prefix("func ")?;
    let rest = rest.trim_start();
    if let Some(receiver_rest) = rest.strip_prefix('(') {
        let receiver_end = receiver_rest.find(')')?;
        let receiver = go_receiver_type_name(&receiver_rest[..receiver_end])?;
        let after_receiver = receiver_rest[receiver_end + 1..].trim_start();
        let method = leading_identifier(after_receiver)?;
        return Some((format!("{receiver}.{method}"), NodeKind::METHOD));
    }

    let name = leading_identifier(rest)?;
    Some((name, NodeKind::FUNCTION))
}

fn parse_go_type_symbol(line: &str) -> Option<(String, NodeKind)> {
    let rest = line.strip_prefix("type ")?;
    let name = leading_identifier(rest)?;
    let after_name = rest[name.len()..].trim_start();
    let kind = if after_name.starts_with("struct") {
        NodeKind::STRUCT
    } else if after_name.starts_with("interface") {
        NodeKind::INTERFACE
    } else {
        NodeKind::TYPEDEF
    };
    Some((name, kind))
}

fn leading_identifier(value: &str) -> Option<String> {
    let mut chars = value.char_indices();
    let (_, first) = chars.next()?;
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    let mut end = first.len_utf8();
    for (index, ch) in chars {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            end = index + ch.len_utf8();
        } else {
            break;
        }
    }
    Some(value[..end].to_string())
}

fn go_receiver_type_name(receiver: &str) -> Option<String> {
    let type_part = receiver.split_whitespace().last()?;
    let cleaned = type_part
        .trim_start_matches('*')
        .trim_start_matches('&')
        .trim_start_matches("[]");
    leading_identifier(cleaned.rsplit('.').next().unwrap_or(cleaned))
}

fn text_only_symbol_node_id(path: &Path, symbol: &TextOnlySymbol) -> NodeId {
    NodeId(generate_id(&format!(
        "{}:{}:{}",
        path.to_string_lossy(),
        symbol.name,
        symbol.line
    )))
}

fn go_symbol_access(name: &str) -> AccessKind {
    let terminal = name.rsplit('.').next().unwrap_or(name);
    if terminal
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
    {
        AccessKind::Public
    } else {
        AccessKind::Private
    }
}

fn append_text_only_framework_routes(
    path: &Path,
    language_name: &str,
    source: &str,
    file_id: NodeId,
    local_storage: &mut IntermediateStorage,
) {
    for route in collect_framework_routes(path, language_name, source) {
        let route = route.with_extraction_provenance("text_only");
        let route_node = framework_route_node(file_id, &route);
        let route_node_id = route_node.id;
        local_storage.nodes.push(route_node);
        local_storage
            .component_access
            .push((route_node_id, AccessKind::Public));
        local_storage
            .edges
            .push(framework_route_member_edge(file_id, route_node_id, &route));
        local_storage
            .occurrences
            .push(framework_route_occurrence(file_id, route_node_id, &route));
    }
}

fn append_text_only_tauri_invocations(
    language_name: &str,
    source: &str,
    file_id: NodeId,
    local_storage: &mut IntermediateStorage,
) {
    if language_name != "svelte" {
        return;
    }
    if !has_tauri_invoke_evidence(source) {
        return;
    }

    for invocation in collect_tauri_command_invocations(source) {
        let command_node = tauri_command_node(file_id, &invocation.command, invocation.line);
        let command_node_id = command_node.id;
        local_storage.nodes.push(command_node);
        local_storage.edges.push(tauri_command_invoke_edge(
            file_id,
            file_id,
            command_node_id,
            &invocation,
            index_feature_flags(),
        ));
        local_storage.occurrences.push(Occurrence {
            element_id: command_node_id.0,
            kind: OccurrenceKind::REFERENCE,
            location: SourceLocation {
                file_node_id: file_id,
                start_line: invocation.line,
                start_col: invocation.col,
                end_line: invocation.line,
                end_col: invocation
                    .col
                    .saturating_add(invocation.command.len() as u32),
            },
        });
    }
}

fn has_tauri_invoke_evidence(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    lower.contains("@tauri-apps/api/core")
        || lower.contains("@tauri-apps/api/tauri")
        || lower.contains("__tauri__")
}

fn index_openapi_schema_file(path: &Path, source: &str) -> Result<Option<IntermediateStorage>> {
    if !looks_like_openapi_schema(source) {
        return Ok(None);
    }
    let endpoints = parse_openapi_endpoints(source)?;
    if endpoints.is_empty() {
        return Ok(None);
    }

    let mut local_storage = IntermediateStorage::default();
    let (file_node, _file_name, file_id) = file_node_from_source(path, source);
    local_storage.files.push(codestory_store::FileInfo {
        id: file_id.0,
        path: path.to_path_buf(),
        language: "openapi".to_string(),
        modification_time: file_modification_time(path),
        indexed: true,
        complete: true,
        line_count: source.lines().count() as u32,
        file_role: codestory_store::FileRole::classify_path(path),
    });
    local_storage.nodes.push(file_node);

    let mut seen = HashSet::new();
    for endpoint in endpoints {
        if !seen.insert((endpoint.method.clone(), endpoint.path.clone())) {
            continue;
        }
        let node_id = schema_endpoint_node_id(&endpoint.method, &endpoint.path);
        let label = schema_endpoint_label(&endpoint.method, &endpoint.path);
        local_storage.nodes.push(Node {
            id: node_id,
            kind: NodeKind::FUNCTION,
            serialized_name: label.clone(),
            qualified_name: Some(format!("openapi::{label}")),
            canonical_id: Some(format!("openapi:endpoint:{label}")),
            file_node_id: Some(file_id),
            start_line: Some(endpoint.line),
            start_col: Some(1),
            end_line: Some(endpoint.line),
            end_col: Some(label.len().max(1) as u32),
        });
        local_storage
            .component_access
            .push((node_id, AccessKind::Public));
        local_storage.edges.push(Edge {
            id: EdgeId(generate_edge_id(file_id.0, node_id.0, EdgeKind::MEMBER)),
            source: file_id,
            target: node_id,
            kind: EdgeKind::MEMBER,
            file_node_id: Some(file_id),
            line: Some(endpoint.line),
            certainty: Some(ResolutionCertainty::Certain),
            ..Default::default()
        });
        local_storage.occurrences.push(Occurrence {
            element_id: node_id.0,
            kind: OccurrenceKind::DEFINITION,
            location: SourceLocation {
                file_node_id: file_id,
                start_line: endpoint.line,
                start_col: 1,
                end_line: endpoint.line,
                end_col: label.len().max(1) as u32,
            },
        });
    }

    Ok(Some(local_storage))
}

/// Return whether text contains the minimum OpenAPI/Swagger markers.
pub fn looks_like_openapi_schema(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    contains_openapi_schema_marker(&lower) && contains_openapi_paths_marker(&lower)
}

fn contains_openapi_schema_marker(lower_source: &str) -> bool {
    lower_source.contains("\"openapi\"")
        || lower_source.contains("openapi:")
        || lower_source.contains("\"swagger\"")
}

fn contains_openapi_paths_marker(lower_source: &str) -> bool {
    lower_source.contains("\"paths\"") || lower_source.contains("paths:")
}

fn parse_openapi_endpoints(source: &str) -> Result<Vec<OpenApiEndpoint>> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(source) {
        return Ok(parse_openapi_json_endpoints(source, &value));
    }
    Ok(parse_openapi_yaml_endpoints(source))
}

fn parse_openapi_json_endpoints(source: &str, value: &serde_json::Value) -> Vec<OpenApiEndpoint> {
    let Some(paths) = value.get("paths").and_then(|value| value.as_object()) else {
        return Vec::new();
    };
    let mut endpoints = Vec::new();
    for (path, methods) in paths {
        let Some(methods) = methods.as_object() else {
            continue;
        };
        for method in methods.keys() {
            if is_http_method(method) {
                endpoints.push(OpenApiEndpoint {
                    method: method.to_ascii_uppercase(),
                    path: path.clone(),
                    line: find_endpoint_line(source, path, method),
                });
            }
        }
    }
    endpoints
}

fn parse_openapi_yaml_endpoints(source: &str) -> Vec<OpenApiEndpoint> {
    let mut endpoints = Vec::new();
    let mut inside_paths = false;
    let mut current_path: Option<String> = None;
    let mut current_path_indent = 0usize;

    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        let indent = line.len().saturating_sub(line.trim_start().len());
        if trimmed == "paths:" {
            inside_paths = true;
            current_path = None;
            current_path_indent = indent;
            continue;
        }
        if inside_paths && indent <= current_path_indent && !trimmed.starts_with('/') {
            break;
        }
        if !inside_paths {
            continue;
        }
        if let Some(path) = trimmed
            .strip_suffix(':')
            .filter(|value| value.starts_with('/'))
        {
            current_path = Some(path.trim_matches('"').trim_matches('\'').to_string());
            current_path_indent = indent;
            continue;
        }
        let Some(path) = current_path.as_ref() else {
            continue;
        };
        let method = trimmed.trim_end_matches(':');
        if indent > current_path_indent && is_http_method(method) {
            endpoints.push(OpenApiEndpoint {
                method: method.to_ascii_uppercase(),
                path: path.clone(),
                line: index as u32 + 1,
            });
        }
    }
    endpoints
}

fn find_endpoint_line(source: &str, path: &str, method: &str) -> u32 {
    let method = method.to_ascii_lowercase();
    let mut path_seen = false;
    for (index, line) in source.lines().enumerate() {
        if line.contains(path) {
            path_seen = true;
        }
        if path_seen && line.to_ascii_lowercase().contains(&format!("\"{method}\"")) {
            return index as u32 + 1;
        }
    }
    source
        .lines()
        .position(|line| line.contains(path))
        .map(|index| index as u32 + 1)
        .unwrap_or(1)
}

fn is_http_method(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "trace"
    )
}

fn schema_endpoint_label(method: &str, path: &str) -> String {
    format!(
        "{} {}",
        method.to_ascii_uppercase(),
        normalize_api_path(path)
    )
}

fn schema_endpoint_node_id(method: &str, path: &str) -> NodeId {
    NodeId(generate_id(&format!(
        "openapi:endpoint:{}",
        schema_endpoint_label(method, path)
    )))
}

fn normalize_api_path(path: &str) -> String {
    let trimmed = path.trim().trim_matches('"').trim_matches('\'');
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn normalize_framework_route_path(path: &str) -> String {
    let path = normalize_api_path(path);
    let segments = path
        .split('/')
        .map(|segment| {
            if segment.is_empty() {
                return String::new();
            }
            if segment.starts_with("[[...") && segment.ends_with("]]") {
                return format!(
                    ":{}",
                    segment.trim_start_matches("[[...").trim_end_matches("]]")
                );
            }
            if segment.starts_with("[...") && segment.ends_with(']') {
                return format!(
                    ":{}",
                    segment.trim_start_matches("[...").trim_end_matches(']')
                );
            }
            if segment.starts_with("[[") && segment.ends_with("]]") {
                return format!(
                    ":{}",
                    segment.trim_start_matches("[[").trim_end_matches("]]")
                );
            }
            if segment.starts_with('[') && segment.ends_with(']') {
                return format!(":{}", segment.trim_start_matches('[').trim_end_matches(']'));
            }
            if segment.starts_with('$') && segment.len() > 1 {
                return format!(":{}", segment.trim_start_matches('$'));
            }
            if segment.starts_with('{') && segment.ends_with('}') && segment.len() > 2 {
                return format!(":{}", segment.trim_start_matches('{').trim_end_matches('}'));
            }
            segment.to_string()
        })
        .collect::<Vec<_>>()
        .join("/");
    if segments == "/" || segments.is_empty() {
        "/".to_string()
    } else {
        segments
    }
}

fn collect_framework_routes(path: &Path, language_name: &str, source: &str) -> Vec<FrameworkRoute> {
    let mut routes = Vec::new();
    let code_lines = route_code_lines(language_name, source);
    let code_source = code_lines.join("\n");
    let lower_code_source = code_source.to_ascii_lowercase();
    let has_koa_router =
        lower_code_source.contains("@koa/router") || lower_code_source.contains("koa-router");
    let has_hono = lower_code_source.contains("hono");
    let has_ktor = lower_code_source.contains("ktor");
    let has_vapor = lower_code_source.contains("vapor");
    let has_shelf =
        lower_code_source.contains("package:shelf") || lower_code_source.contains("shelf_router");
    let react_router_object_route_lines =
        react_router_object_route_lines(&code_lines, &code_source);
    for (index, line) in code_lines.iter().enumerate() {
        let line_number = index as u32 + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match language_name {
            "javascript" | "typescript" => {
                collect_express_route(trimmed, line_number, &mut routes);
                collect_fastify_route(trimmed, line_number, &mut routes);
                collect_koa_route(trimmed, line_number, &mut routes, has_koa_router);
                collect_hono_route(trimmed, line_number, &mut routes, has_hono);
                collect_react_route(
                    trimmed,
                    line_number,
                    &mut routes,
                    react_router_object_route_lines.contains(&line_number),
                );
                collect_sveltekit_server_route(path, trimmed, line_number, &mut routes);
                collect_next_route_handler(path, trimmed, line_number, &mut routes);
                collect_astro_endpoint_route(path, trimmed, line_number, &mut routes);
                collect_nuxt_server_route(path, trimmed, line_number, &mut routes);
            }
            "python" => collect_python_route(trimmed, line_number, &mut routes),
            "java" => collect_spring_route(trimmed, line_number, &mut routes),
            "rust" => collect_rust_web_route(trimmed, line_number, &mut routes),
            "go" => collect_go_route(trimmed, line_number, &mut routes, &code_source),
            "ruby" => collect_rails_route(trimmed, line_number, &mut routes),
            "php" => collect_laravel_route(trimmed, line_number, &mut routes),
            "csharp" => collect_aspnet_route(trimmed, line_number, &mut routes),
            "kotlin" => collect_ktor_route(trimmed, line_number, &mut routes, has_ktor),
            "swift" => collect_vapor_route(trimmed, line_number, &mut routes, has_vapor),
            "dart" => collect_shelf_route(trimmed, line_number, &mut routes, has_shelf),
            "vue" => collect_vue_route(trimmed, line_number, &mut routes),
            "astro" => collect_astro_endpoint_route(path, trimmed, line_number, &mut routes),
            _ => {}
        }
    }
    if matches!(language_name, "javascript" | "typescript") {
        collect_next_file_route(path, &code_source, &mut routes);
        collect_remix_file_route(path, &code_source, &mut routes);
        collect_nestjs_routes(&code_source, &mut routes);
    }
    if language_name == "svelte" {
        collect_sveltekit_page_route(path, 1, &mut routes);
    }
    if language_name == "vue" {
        collect_nuxt_page_route(path, &mut routes);
    }
    if language_name == "astro" {
        collect_astro_page_route(path, &mut routes);
    }
    dedupe_framework_routes(routes)
}

fn route_code_lines(language_name: &str, source: &str) -> Vec<String> {
    if route_language_uses_c_style_comments(language_name) {
        strip_c_style_comments(source)
    } else {
        source
            .lines()
            .map(|line| {
                let code = code_before_line_comment(line);
                match language_name {
                    "python" | "ruby" => code_before_hash_comment(code).to_string(),
                    _ => code.to_string(),
                }
            })
            .collect()
    }
}

fn route_language_uses_c_style_comments(language_name: &str) -> bool {
    if let Some(extraction) = languages::extraction_for_language(language_name) {
        return extraction.route_comments_are_c_style;
    }
    matches!(language_name, "typescript" | "dart" | "vue" | "astro")
}

fn strip_c_style_comments(source: &str) -> Vec<String> {
    let mut lines = vec![String::new()];
    let mut chars = source.chars().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut in_block_comment = false;

    while let Some(ch) = chars.next() {
        if in_block_comment {
            if ch == '*'
                && let Some('/') = chars.peek()
            {
                chars.next();
                in_block_comment = false;
            } else if ch == '\n' {
                lines.push(String::new());
            }
            continue;
        }

        if let Some(active_quote) = quote {
            if ch == '\n' {
                lines.push(String::new());
                if active_quote != '`' {
                    quote = None;
                }
                escaped = false;
                continue;
            }
            lines
                .last_mut()
                .expect("strip_c_style_comments keeps one current line")
                .push(ch);
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }

        if ch == '\n' {
            lines.push(String::new());
            continue;
        }
        if ch == '/'
            && let Some(next) = chars.peek().copied()
        {
            if next == '/' {
                chars.next();
                for rest in chars.by_ref() {
                    if rest == '\n' {
                        lines.push(String::new());
                        break;
                    }
                }
                continue;
            }
            if next == '*' {
                chars.next();
                in_block_comment = true;
                continue;
            }
        }
        if matches!(ch, '"' | '\'' | '`') {
            quote = Some(ch);
        }
        lines
            .last_mut()
            .expect("strip_c_style_comments keeps one current line")
            .push(ch);
    }

    lines
}

fn has_react_router_context(source: &str) -> bool {
    source.contains("react-router")
        || source.contains("createBrowserRouter")
        || source.contains("createHashRouter")
        || source.contains("createMemoryRouter")
        || source.contains("createRoutesFromElements")
        || source.contains("RouterProvider")
        || source.contains("RouteObject")
}

fn react_router_object_route_lines(code_lines: &[String], code_source: &str) -> HashSet<u32> {
    let mut route_lines = HashSet::new();
    if !has_react_router_context(code_source) {
        return route_lines;
    }

    let mut in_router_config = false;
    let mut delimiter_depth = 0i32;
    for (index, line) in code_lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let starts_router_config = starts_react_router_object_config(trimmed);
        if starts_router_config {
            in_router_config = true;
        }
        if in_router_config && trimmed.contains("path:") {
            route_lines.insert(index as u32 + 1);
        }
        if in_router_config {
            delimiter_depth += react_router_config_depth_delta(trimmed);
            if delimiter_depth <= 0 && trimmed.ends_with(';') {
                in_router_config = false;
                delimiter_depth = 0;
            }
        }
    }

    route_lines
}

fn starts_react_router_object_config(line: &str) -> bool {
    let route_object_declaration = line.contains("RouteObject")
        && !line.starts_with("import ")
        && (line.contains('=') || line.contains(':') || line.contains("satisfies"));
    line.contains("createBrowserRouter(")
        || line.contains("createHashRouter(")
        || line.contains("createMemoryRouter(")
        || route_object_declaration
}

fn react_router_config_depth_delta(line: &str) -> i32 {
    line.chars().fold(0, |depth, ch| match ch {
        '(' | '[' | '{' => depth + 1,
        ')' | ']' | '}' => depth - 1,
        _ => depth,
    })
}

fn collect_express_route(line: &str, line_number: u32, routes: &mut Vec<FrameworkRoute>) {
    for receiver in ["app", "router"] {
        for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
            let needle = format!("{receiver}.{method}(");
            if let Some(args) = line.split_once(&needle).map(|(_, tail)| tail)
                && let Some(path) = first_quoted_string(args)
            {
                routes.push(FrameworkRoute::new(
                    "express",
                    method.to_ascii_uppercase(),
                    path.clone(),
                    route_handler_after_path(args, &path),
                    line_number,
                    "heuristic",
                ));
            }
        }
    }
}

fn collect_fastify_route(line: &str, line_number: u32, routes: &mut Vec<FrameworkRoute>) {
    for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
        let needle = format!(".{method}(");
        if (line.contains(&format!("fastify{needle}")) || line.contains(&format!("server{needle}")))
            && let Some(path) = first_quoted_string(line)
        {
            routes.push(FrameworkRoute::new(
                "fastify",
                method.to_ascii_uppercase(),
                path.clone(),
                route_handler_after_path(line, &path),
                line_number,
                "heuristic",
            ));
        }
    }

    if line.contains(".route(") && line.contains("method") && line.contains("url") {
        let method = value_after_key(line, "method").unwrap_or_else(|| "ROUTE".to_string());
        if let Some(path) = value_after_key(line, "url") {
            routes.push(FrameworkRoute::new(
                "fastify",
                method.to_ascii_uppercase(),
                path.clone(),
                value_after_key(line, "handler").or_else(|| route_handler_after_path(line, &path)),
                line_number,
                "heuristic",
            ));
        }
    }
}

fn collect_koa_route(
    line: &str,
    line_number: u32,
    routes: &mut Vec<FrameworkRoute>,
    has_koa_router: bool,
) {
    if !has_koa_router {
        return;
    }
    for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
        let needle = format!(".{method}(");
        if (line.contains(&format!("router{needle}"))
            || line.contains(&format!("koaRouter{needle}")))
            && let Some(path) = first_quoted_string(line)
        {
            routes.push(FrameworkRoute::new(
                "koa",
                method.to_ascii_uppercase(),
                path.clone(),
                route_handler_after_path(line, &path),
                line_number,
                "heuristic",
            ));
        }
    }
}

fn collect_hono_route(
    line: &str,
    line_number: u32,
    routes: &mut Vec<FrameworkRoute>,
    has_hono: bool,
) {
    if !has_hono {
        return;
    }
    for method in ["get", "post", "put", "patch", "delete", "all"] {
        let needle = format!(".{method}(");
        if (line.contains(&format!("app{needle}")) || line.contains(&format!("route{needle}")))
            && let Some(path) = first_quoted_string(line)
        {
            routes.push(FrameworkRoute::new(
                "hono",
                method.to_ascii_uppercase(),
                path.clone(),
                route_handler_after_path(line, &path),
                line_number,
                "heuristic",
            ));
        }
    }
}

fn collect_react_route(
    line: &str,
    line_number: u32,
    routes: &mut Vec<FrameworkRoute>,
    react_router_context: bool,
) {
    if let Some(path) = react_route_path(line, react_router_context) {
        routes.push(FrameworkRoute::new(
            "react-router",
            "GET".to_string(),
            path,
            None,
            line_number,
            "heuristic",
        ));
    }
}

fn react_route_path(line: &str, react_router_context: bool) -> Option<String> {
    let jsx_route = line.contains("<Route") && line.contains("path");
    let object_route = react_router_context && line.contains("path:");
    if !(jsx_route || object_route) {
        return None;
    }
    value_after_key(line, "path").or_else(|| first_quoted_string(line))
}

fn collect_sveltekit_server_route(
    path: &Path,
    line: &str,
    line_number: u32,
    routes: &mut Vec<FrameworkRoute>,
) {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if file_name != "+server.ts" && file_name != "+server.js" {
        return;
    }
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
        if line.contains(&format!("export function {method}"))
            || line.contains(&format!("export async function {method}"))
            || line.contains(&format!("export const {method}"))
        {
            routes.push(FrameworkRoute::new(
                "sveltekit",
                method.to_string(),
                sveltekit_route_path(path),
                Some(method.to_string()),
                line_number,
                "file_convention",
            ));
        }
    }
}

fn collect_sveltekit_page_route(path: &Path, line_number: u32, routes: &mut Vec<FrameworkRoute>) {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if file_name != "+page.svelte" {
        return;
    }
    routes.push(FrameworkRoute::new(
        "sveltekit",
        "GET".to_string(),
        sveltekit_route_path(path),
        None,
        line_number,
        "file_convention",
    ));
}

fn collect_next_route_handler(
    path: &Path,
    line: &str,
    line_number: u32,
    routes: &mut Vec<FrameworkRoute>,
) {
    if !is_file_named(path, "route") || !path_has_component(path, "app") {
        return;
    }
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
        if exports_named_handler(line, method) {
            routes.push(FrameworkRoute::new(
                "nextjs",
                method.to_string(),
                nextjs_route_path(path),
                Some(method.to_string()),
                line_number,
                "file_convention",
            ));
        }
    }
}

fn collect_next_file_route(path: &Path, source: &str, routes: &mut Vec<FrameworkRoute>) {
    if path_has_component(path, "app") {
        let file_stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if file_stem == "page" {
            routes.push(FrameworkRoute::new(
                "nextjs",
                "GET".to_string(),
                nextjs_route_path(path),
                default_export_handler(source),
                1,
                "file_convention",
            ));
        }
    }

    if path_has_component(path, "pages") {
        let route_path = pages_file_route_path(path, "pages");
        if !matches!(route_path.as_str(), "/_app" | "/_document" | "/_error") {
            routes.push(FrameworkRoute::new(
                "nextjs",
                "GET".to_string(),
                route_path,
                default_export_handler(source),
                1,
                "file_convention",
            ));
        }
    }
}

fn collect_remix_file_route(path: &Path, source: &str, routes: &mut Vec<FrameworkRoute>) {
    if !path_has_component(path, "routes") {
        return;
    }
    if !has_remix_route_evidence(path, source) {
        return;
    }
    let route_path = remix_route_path(path);
    if route_path.is_empty() {
        return;
    }
    if source.contains("loader") {
        routes.push(FrameworkRoute::new(
            "remix",
            "GET".to_string(),
            route_path.clone(),
            Some("loader".to_string()),
            1,
            "file_convention",
        ));
    }
    if source.contains("action") {
        routes.push(FrameworkRoute::new(
            "remix",
            "POST".to_string(),
            route_path.clone(),
            Some("action".to_string()),
            1,
            "file_convention",
        ));
    }
    if has_remix_default_route_evidence(source)
        && routes
            .iter()
            .all(|route| route.framework != "remix" || route.raw_path != route_path)
    {
        routes.push(FrameworkRoute::new(
            "remix",
            "GET".to_string(),
            route_path,
            default_export_handler(source),
            1,
            "file_convention",
        ));
    }
}

fn has_remix_route_evidence(path: &Path, source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    lower.contains("@remix-run/")
        || lower.contains(" from \"@remix-run/")
        || lower.contains(" from '@remix-run/")
        || lower.contains("export async function loader")
        || lower.contains("export function loader")
        || lower.contains("export const loader")
        || lower.contains("export async function action")
        || lower.contains("export function action")
        || lower.contains("export const action")
        || path
            .components()
            .any(|component| component.as_os_str().to_string_lossy() == "app")
}

fn has_remix_default_route_evidence(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    lower.contains("@remix-run/")
        || lower.contains("export default")
        || lower.contains("export async function loader")
        || lower.contains("export function loader")
        || lower.contains("export const loader")
        || lower.contains("export async function action")
        || lower.contains("export function action")
        || lower.contains("export const action")
}

fn collect_astro_endpoint_route(
    path: &Path,
    line: &str,
    line_number: u32,
    routes: &mut Vec<FrameworkRoute>,
) {
    if !path_has_component(path, "pages") {
        return;
    }
    if !path_has_component(path, "src") {
        return;
    }
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
        if exports_named_handler(line, method) {
            routes.push(FrameworkRoute::new(
                "astro",
                method.to_string(),
                pages_file_route_path(path, "pages"),
                Some(method.to_string()),
                line_number,
                "file_convention",
            ));
        }
    }
}

fn collect_astro_page_route(path: &Path, routes: &mut Vec<FrameworkRoute>) {
    if !path_has_component(path, "pages") || is_file_named(path, "404") {
        return;
    }
    routes.push(FrameworkRoute::new(
        "astro",
        "GET".to_string(),
        pages_file_route_path(path, "pages"),
        None,
        1,
        "file_convention",
    ));
}

fn collect_nuxt_server_route(
    path: &Path,
    line: &str,
    line_number: u32,
    routes: &mut Vec<FrameworkRoute>,
) {
    if !path_has_component(path, "server") {
        return;
    }
    if line.contains("defineEventHandler") || line.contains("export default") {
        routes.push(FrameworkRoute::new(
            "nuxt",
            nuxt_server_method(path),
            nuxt_server_route_path(path),
            Some("default".to_string()),
            line_number,
            "file_convention",
        ));
    }
}

fn collect_nuxt_page_route(path: &Path, routes: &mut Vec<FrameworkRoute>) {
    if !path_has_component(path, "pages") {
        return;
    }
    routes.push(FrameworkRoute::new(
        "nuxt",
        "GET".to_string(),
        pages_file_route_path(path, "pages"),
        None,
        1,
        "file_convention",
    ));
}

fn collect_nestjs_routes(source: &str, routes: &mut Vec<FrameworkRoute>) {
    let mut controller_prefix = String::new();
    let mut pending: Option<(String, String, u32)> = None;
    for (index, line) in source.lines().enumerate() {
        let line_number = index as u32 + 1;
        let trimmed = code_before_line_comment(line).trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("@Controller") {
            controller_prefix = first_quoted_string(trimmed).unwrap_or_default();
            continue;
        }
        for (annotation, method) in [
            ("@Get", "GET"),
            ("@Post", "POST"),
            ("@Put", "PUT"),
            ("@Patch", "PATCH"),
            ("@Delete", "DELETE"),
            ("@All", "ROUTE"),
        ] {
            if trimmed.starts_with(annotation) {
                let child_path = first_quoted_string(trimmed).unwrap_or_default();
                pending = Some((
                    method.to_string(),
                    join_route_paths(&controller_prefix, &child_path),
                    line_number,
                ));
                break;
            }
        }
        if let Some((method, route_path, route_line)) = pending.take() {
            if let Some(handler) = typescript_method_name(trimmed) {
                routes.push(FrameworkRoute::new(
                    "nestjs",
                    method,
                    route_path,
                    Some(handler),
                    route_line,
                    "decorator",
                ));
            } else {
                pending = Some((method, route_path, route_line));
            }
        }
    }
}

fn collect_python_route(line: &str, line_number: u32, routes: &mut Vec<FrameworkRoute>) {
    let lower = line.to_ascii_lowercase();
    if (lower.starts_with("@app.route") || lower.starts_with("@blueprint.route"))
        && let Some(path) = first_quoted_string(line)
    {
        let method = route_methods_literal(line).unwrap_or_else(|| "GET".to_string());
        routes.push(FrameworkRoute::new(
            "flask",
            method,
            path,
            None,
            line_number,
            "decorator",
        ));
    }
    for method in ["get", "post", "put", "patch", "delete"] {
        if lower.starts_with(&format!("@app.{method}("))
            && let Some(path) = first_quoted_string(line)
        {
            routes.push(FrameworkRoute::new(
                "fastapi",
                method.to_ascii_uppercase(),
                path,
                None,
                line_number,
                "decorator",
            ));
        }
    }
    if (line.contains("path(") || line.contains("re_path("))
        && let Some(route_path) = first_quoted_string(line)
    {
        routes.push(FrameworkRoute::new(
            "django",
            "ROUTE".to_string(),
            route_path.clone(),
            route_handler_after_path(line, &route_path),
            line_number,
            "heuristic",
        ));
    }
}

fn collect_spring_route(line: &str, line_number: u32, routes: &mut Vec<FrameworkRoute>) {
    let mappings = [
        ("@GetMapping", "GET"),
        ("@PostMapping", "POST"),
        ("@PutMapping", "PUT"),
        ("@PatchMapping", "PATCH"),
        ("@DeleteMapping", "DELETE"),
        ("@RequestMapping", "ROUTE"),
    ];
    for (annotation, method) in mappings {
        if line.contains(annotation)
            && let Some(path) = first_quoted_string(line)
        {
            routes.push(FrameworkRoute::new(
                "spring",
                method.to_string(),
                path,
                None,
                line_number,
                "annotation",
            ));
        }
    }
}

fn collect_rust_web_route(line: &str, line_number: u32, routes: &mut Vec<FrameworkRoute>) {
    if line.contains(".route(")
        && let Some(path) = first_quoted_string(line)
    {
        let method = ["get", "post", "put", "patch", "delete"]
            .iter()
            .find(|method| line.contains(&format!("{method}(")))
            .map(|method| method.to_ascii_uppercase())
            .unwrap_or_else(|| "ROUTE".to_string());
        let framework = if line.contains("web::") {
            "actix"
        } else {
            "axum"
        };
        routes.push(FrameworkRoute::new(
            framework,
            method,
            path,
            handler_inside_last_call(line),
            line_number,
            "heuristic",
        ));
    }
    for method in ["get", "post", "put", "patch", "delete"] {
        let attr = format!("#[{method}(");
        if line.contains(&attr)
            && let Some(path) = first_quoted_string(line)
        {
            routes.push(FrameworkRoute::new(
                "rocket",
                method.to_ascii_uppercase(),
                path,
                None,
                line_number,
                "attribute",
            ));
        }
    }
}

fn collect_go_route(line: &str, line_number: u32, routes: &mut Vec<FrameworkRoute>, source: &str) {
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
        let needle = format!(".{method}(");
        if line.contains(&needle)
            && let Some(path) = first_quoted_string(line)
            && let Some(framework) = go_route_framework(line, source)
        {
            routes.push(FrameworkRoute::new(
                framework,
                method.to_string(),
                path.clone(),
                route_handler_after_path(line, &path),
                line_number,
                "heuristic",
            ));
        }

        let chi_needle = format!(".{}(", method_title_case(method));
        if line.contains(&chi_needle)
            && let Some(path) = first_quoted_string(line)
            && let Some(framework) = go_route_framework(line, source)
        {
            routes.push(FrameworkRoute::new(
                framework,
                method.to_string(),
                path.clone(),
                route_handler_after_path(line, &path),
                line_number,
                "heuristic",
            ));
        }
    }
}

fn collect_rails_route(line: &str, line_number: u32, routes: &mut Vec<FrameworkRoute>) {
    for method in ["get", "post", "put", "patch", "delete", "resources"] {
        if line.trim_start().starts_with(method)
            && let Some(path) = first_quoted_string(line)
        {
            routes.push(FrameworkRoute::new(
                "rails",
                method.to_ascii_uppercase(),
                path,
                value_after_key(line, "to"),
                line_number,
                "heuristic",
            ));
        }
    }
}

fn collect_laravel_route(line: &str, line_number: u32, routes: &mut Vec<FrameworkRoute>) {
    for method in ["get", "post", "put", "patch", "delete", "any"] {
        let needle = format!("Route::{method}(");
        if line.contains(&needle)
            && let Some(path) = first_quoted_string(line)
        {
            routes.push(FrameworkRoute::new(
                "laravel",
                method.to_ascii_uppercase(),
                path,
                None,
                line_number,
                "heuristic",
            ));
        }
    }
}

fn collect_ktor_route(
    line: &str,
    line_number: u32,
    routes: &mut Vec<FrameworkRoute>,
    has_ktor: bool,
) {
    if !has_ktor {
        return;
    }
    for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
        let attr = format!("@{method}(");
        if line.contains(&attr)
            && let Some(path) = first_quoted_string(line)
        {
            routes.push(FrameworkRoute::new(
                "ktor",
                method.to_ascii_uppercase(),
                path,
                None,
                line_number,
                "annotation",
            ));
            continue;
        }
        for prefix in ["", "."] {
            let needle = format!("{prefix}{method}(");
            if line.contains(&needle)
                && let Some(path) = first_quoted_string(line)
            {
                routes.push(FrameworkRoute::new(
                    "ktor",
                    method.to_ascii_uppercase(),
                    path.clone(),
                    route_handler_after_path(line, &path),
                    line_number,
                    "heuristic",
                ));
            }
        }
    }
}

fn collect_vapor_route(
    line: &str,
    line_number: u32,
    routes: &mut Vec<FrameworkRoute>,
    has_vapor: bool,
) {
    if !has_vapor && !line.contains("routes.") && !line.contains("app.") {
        return;
    }
    for method in ["get", "post", "put", "patch", "delete"] {
        let dotted = format!(".{method}(");
        let bare = format!("{method}(");
        if (line.contains(&dotted) || line.trim_start().starts_with(&bare))
            && let Some(path) = first_quoted_string(line)
        {
            routes.push(FrameworkRoute::new(
                "vapor",
                method.to_ascii_uppercase(),
                path.clone(),
                value_after_key(line, "use"),
                line_number,
                "heuristic",
            ));
        }
    }
}

fn collect_shelf_route(
    line: &str,
    line_number: u32,
    routes: &mut Vec<FrameworkRoute>,
    has_shelf: bool,
) {
    if !has_shelf && !line.contains("router.") && !line.contains("Router()") {
        return;
    }
    for method in ["get", "post", "put", "patch", "delete", "head"] {
        let needle = format!(".{method}(");
        if line.contains(&needle)
            && let Some(path) = first_quoted_string(line)
        {
            routes.push(FrameworkRoute::new(
                "shelf",
                method.to_ascii_uppercase(),
                path.clone(),
                route_handler_after_path(line, &path),
                line_number,
                "heuristic",
            ));
        }
    }
}

fn collect_aspnet_route(line: &str, line_number: u32, routes: &mut Vec<FrameworkRoute>) {
    let attrs = [
        ("[HttpGet", "GET"),
        ("[HttpPost", "POST"),
        ("[HttpPut", "PUT"),
        ("[HttpPatch", "PATCH"),
        ("[HttpDelete", "DELETE"),
        ("[Route", "ROUTE"),
    ];
    for (attr, method) in attrs {
        if line.contains(attr)
            && let Some(path) = first_quoted_string(line)
        {
            routes.push(FrameworkRoute::new(
                "aspnet",
                method.to_string(),
                path,
                None,
                line_number,
                "attribute",
            ));
        }
    }
}

fn collect_vue_route(line: &str, line_number: u32, routes: &mut Vec<FrameworkRoute>) {
    if line.contains("path:")
        && let Some(path) = value_after_key(line, "path")
    {
        routes.push(FrameworkRoute::new(
            "vue-router",
            "GET".to_string(),
            path,
            value_after_key(line, "name"),
            line_number,
            "heuristic",
        ));
    }
}

fn first_quoted_string(value: &str) -> Option<String> {
    let mut quote = None;
    let mut start = 0;
    for (index, ch) in value.char_indices() {
        if ch == '"' || ch == '\'' {
            if quote.is_none() {
                quote = Some(ch);
                start = index + ch.len_utf8();
            } else if quote == Some(ch) {
                return Some(value[start..index].to_string());
            }
        }
    }
    None
}

fn value_after_key(line: &str, key: &str) -> Option<String> {
    let (_, tail) = line.split_once(key)?;
    first_quoted_string(tail.split_once(':').map(|(_, value)| value).unwrap_or(tail))
}

fn path_has_component(path: &Path, expected: &str) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_string_lossy() == expected)
}

fn is_file_named(path: &Path, expected_stem: &str) -> bool {
    path.file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == expected_stem)
}

fn exports_named_handler(line: &str, method: &str) -> bool {
    line.contains(&format!("export function {method}"))
        || line.contains(&format!("export async function {method}"))
        || line.contains(&format!("export const {method}"))
}

fn default_export_handler(source: &str) -> Option<String> {
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(tail) = trimmed
            .strip_prefix("export default function ")
            .or_else(|| trimmed.strip_prefix("export default async function "))
        {
            let name = tail
                .split(['(', '<', ' '])
                .next()
                .unwrap_or_default()
                .trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn typescript_method_name(line: &str) -> Option<String> {
    let candidate = line
        .split('(')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_start_matches("async ")
        .split_whitespace()
        .last()
        .unwrap_or_default()
        .trim();
    if candidate.is_empty() || candidate.starts_with('@') || candidate.contains('=') {
        None
    } else {
        Some(candidate.to_string())
    }
}

fn route_segments_after(path: &Path, marker: &str) -> Vec<String> {
    let mut seen_marker = false;
    let mut segments = Vec::new();
    for component in path.components() {
        let value = component.as_os_str().to_string_lossy().to_string();
        if value == marker {
            seen_marker = true;
            segments.clear();
            continue;
        }
        if !seen_marker {
            continue;
        }
        segments.push(value);
    }
    segments
}

fn strip_route_extension(segment: &str) -> String {
    let mut segment = segment.to_string();
    for extension in [
        ".tsx", ".ts", ".jsx", ".js", ".mjs", ".cjs", ".vue", ".astro", ".go",
    ] {
        if segment.ends_with(extension) {
            segment.truncate(segment.len().saturating_sub(extension.len()));
            break;
        }
    }
    segment
}

fn file_route_segment(segment: &str) -> Option<String> {
    let segment = strip_route_extension(segment);
    if is_ignored_file_route_segment(&segment) {
        return None;
    }
    let segment = strip_route_method_suffix(&segment).to_string();
    if segment == "index" {
        None
    } else {
        Some(segment)
    }
}

fn is_ignored_file_route_segment(segment: &str) -> bool {
    if segment.is_empty()
        || matches!(segment, "page" | "route" | "layout" | "template" | "+page")
        || segment.starts_with('_')
    {
        return true;
    }
    segment.starts_with('(') && segment.ends_with(')')
}

fn strip_route_method_suffix(segment: &str) -> &str {
    for suffix in [
        ".get", ".post", ".put", ".patch", ".delete", ".head", ".options",
    ] {
        if let Some(stem) = segment.strip_suffix(suffix) {
            return stem;
        }
    }
    segment
}

fn file_route_path_from_segments(segments: &[String]) -> String {
    let parts = segments
        .iter()
        .filter_map(|segment| file_route_segment(segment))
        .collect::<Vec<_>>();
    normalize_api_path(&parts.join("/"))
}

fn pages_file_route_path(path: &Path, marker: &str) -> String {
    file_route_path_from_segments(&route_segments_after(path, marker))
}

fn nextjs_route_path(path: &Path) -> String {
    file_route_path_from_segments(&route_segments_after(path, "app"))
}

fn remix_route_path(path: &Path) -> String {
    let mut segments = route_segments_after(path, "routes");
    if segments.is_empty() {
        segments = route_segments_after(path, "app");
        if segments.first().is_some_and(|segment| segment == "routes") {
            segments.remove(0);
        }
    }
    if segments.is_empty() {
        return String::new();
    }
    let Some(file_name) = segments.pop() else {
        return String::new();
    };
    let stem = strip_route_extension(&file_name);
    let mut route_parts = segments
        .into_iter()
        .filter_map(|segment| file_route_segment(&segment))
        .collect::<Vec<_>>();
    for part in stem.split('.') {
        if part == "_index" || part.is_empty() {
            continue;
        }
        route_parts.push(part.trim_start_matches('_').to_string());
    }
    normalize_api_path(&route_parts.join("/"))
}

fn nuxt_server_method(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    for (suffix, method) in [
        (".get", "GET"),
        (".post", "POST"),
        (".put", "PUT"),
        (".patch", "PATCH"),
        (".delete", "DELETE"),
        (".head", "HEAD"),
        (".options", "OPTIONS"),
    ] {
        if stem.ends_with(suffix) {
            return method.to_string();
        }
    }
    "ROUTE".to_string()
}

fn nuxt_server_route_path(path: &Path) -> String {
    let mut segments = route_segments_after(path, "server");
    if segments
        .first()
        .is_some_and(|segment| segment == "api" || segment == "routes")
    {
        segments.remove(0);
    }
    pages_file_route_path_from_segments(&segments)
}

fn pages_file_route_path_from_segments(segments: &[String]) -> String {
    file_route_path_from_segments(segments)
}

fn join_route_paths(prefix: &str, child: &str) -> String {
    let prefix = prefix.trim_matches('/');
    let child = child.trim_matches('/');
    match (prefix.is_empty(), child.is_empty()) {
        (true, true) => "/".to_string(),
        (true, false) => normalize_framework_route_path(child),
        (false, true) => normalize_framework_route_path(prefix),
        (false, false) => normalize_framework_route_path(&format!("{prefix}/{child}")),
    }
}

fn go_route_framework(line: &str, source: &str) -> Option<&'static str> {
    let lower_source = source.to_ascii_lowercase();
    if lower_source.contains("github.com/gin-gonic/gin") {
        Some("gin")
    } else if lower_source.contains("github.com/labstack/echo") {
        Some("echo")
    } else if lower_source.contains("github.com/gofiber/fiber") {
        Some("fiber")
    } else if lower_source.contains("github.com/go-chi/chi") || line.contains(".Method(") {
        Some("chi")
    } else if line.contains("app.") {
        Some("fiber")
    } else if line.contains("e.") {
        Some("echo")
    } else {
        None
    }
}

fn method_title_case(method: &str) -> String {
    let mut chars = method.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    format!("{}{}", first, chars.as_str().to_ascii_lowercase())
}

fn route_handler_after_path(args: &str, path: &str) -> Option<String> {
    let (_, tail) = args.split_once(path)?;
    tail.trim_matches(|ch: char| ch == '"' || ch == '\'' || ch == ',' || ch.is_whitespace())
        .split([',', ')', ']'])
        .next()
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
        })
        .and_then(|value| value.rsplit('.').next().map(str::to_string))
}

fn handler_inside_last_call(line: &str) -> Option<String> {
    line.rsplit('(')
        .next()
        .and_then(|tail| tail.split(')').next())
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
        .map(str::to_string)
}

fn route_methods_literal(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    ["get", "post", "put", "patch", "delete"]
        .iter()
        .find(|method| {
            lower.contains(&format!("\"{method}\"")) || lower.contains(&format!("'{method}'"))
        })
        .map(|method| method.to_ascii_uppercase())
}

fn sveltekit_route_path(path: &Path) -> String {
    let mut parts = Vec::new();
    let mut in_routes_dir = false;
    for component in path.components() {
        let value = component.as_os_str().to_string_lossy();
        if value == "routes" {
            in_routes_dir = true;
            parts.clear();
            continue;
        }
        if !in_routes_dir {
            continue;
        }
        if value.starts_with('+') {
            continue;
        }
        if value.starts_with('(') && value.ends_with(')') {
            continue;
        }
        let segment = if value.starts_with("[[...") && value.ends_with("]]") {
            format!(
                ":{}",
                value.trim_start_matches("[[...").trim_end_matches("]]")
            )
        } else if value.starts_with("[...") && value.ends_with(']') {
            format!(
                ":{}",
                value.trim_start_matches("[...").trim_end_matches(']')
            )
        } else if value.starts_with("[[") && value.ends_with("]]") {
            format!(":{}", value.trim_start_matches("[[").trim_end_matches("]]"))
        } else if value.starts_with('[') && value.ends_with(']') {
            format!(":{}", value.trim_start_matches('[').trim_end_matches(']'))
        } else {
            value.to_string()
        };
        if !segment.ends_with(".svelte") && !segment.ends_with(".ts") && !segment.ends_with(".js") {
            parts.push(segment);
        }
    }
    normalize_api_path(&parts.join("/"))
}

fn dedupe_framework_routes(routes: Vec<FrameworkRoute>) -> Vec<FrameworkRoute> {
    let mut seen = HashSet::new();
    routes
        .into_iter()
        .filter(|route| {
            seen.insert((
                route.framework,
                route.method.clone(),
                route.path.clone(),
                route.line,
            ))
        })
        .collect()
}

fn framework_route_label(route: &FrameworkRoute) -> String {
    format!(
        "{} {} ({} route; confidence={})",
        route.method, route.path, route.framework, route.confidence
    )
}

fn framework_route_canonical_id(route: &FrameworkRoute) -> String {
    format!(
        "route_endpoint:{}",
        serde_json::json!({
            "kind": "framework_route",
            "framework": route.framework,
            "method": route.method.as_str(),
            "path": route.path.as_str(),
            "raw_path": route.raw_path.as_str(),
            "params": route_params(&route.path),
            "confidence": route.confidence,
            "source_convention": route.source_convention,
            "extraction_provenance": route.extraction_provenance,
            "claim_tier": route.claim_tier,
            "provenance": [
                format!("framework:{}", route.framework),
                format!("extraction:{}", route.extraction_provenance),
                format!("claim_tier:{}", route.claim_tier),
            ],
        })
    )
}

fn route_params(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|segment| {
            let segment = segment.trim();
            segment
                .strip_prefix(':')
                .or_else(|| {
                    segment
                        .strip_prefix('{')
                        .and_then(|value| value.strip_suffix('}'))
                })
                .map(str::to_string)
        })
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn framework_route_node(file_id: NodeId, route: &FrameworkRoute) -> Node {
    let label = framework_route_label(route);
    Node {
        id: NodeId(generate_id(&framework_route_canonical_id(route))),
        kind: NodeKind::FUNCTION,
        serialized_name: label.clone(),
        qualified_name: Some(format!(
            "framework::{}::{} {}",
            route.framework, route.method, route.path
        )),
        canonical_id: Some(framework_route_canonical_id(route)),
        file_node_id: Some(file_id),
        start_line: Some(route.line),
        start_col: Some(1),
        end_line: Some(route.line),
        end_col: Some(label.len().max(1) as u32),
    }
}

fn framework_route_member_edge(
    file_id: NodeId,
    route_node_id: NodeId,
    route: &FrameworkRoute,
) -> Edge {
    Edge {
        id: EdgeId(generate_edge_id(
            file_id.0,
            route_node_id.0,
            EdgeKind::MEMBER,
        )),
        source: file_id,
        target: route_node_id,
        kind: EdgeKind::MEMBER,
        file_node_id: Some(file_id),
        line: Some(route.line),
        certainty: Some(ResolutionCertainty::Certain),
        ..Default::default()
    }
}

fn framework_route_occurrence(
    file_id: NodeId,
    route_node_id: NodeId,
    route: &FrameworkRoute,
) -> Occurrence {
    Occurrence {
        element_id: route_node_id.0,
        kind: OccurrenceKind::DEFINITION,
        location: SourceLocation {
            file_node_id: file_id,
            start_line: route.line,
            start_col: 1,
            end_line: route.line,
            end_col: framework_route_label(route).len().max(1) as u32,
        },
    }
}

fn tauri_command_canonical_id(command: &str) -> String {
    format!("tauri:command:{}", command.trim())
}

fn tauri_command_label(command: &str) -> String {
    format!(
        "tauri command {} (tauri command; confidence=heuristic)",
        command.trim()
    )
}

fn tauri_command_node(file_id: NodeId, command: &str, line: u32) -> Node {
    let canonical_id = tauri_command_canonical_id(command);
    let label = tauri_command_label(command);
    Node {
        id: NodeId(generate_id(&canonical_id)),
        kind: NodeKind::FUNCTION,
        serialized_name: label.clone(),
        qualified_name: Some(format!("framework::tauri::command::{}", command.trim())),
        canonical_id: Some(canonical_id),
        file_node_id: Some(file_id),
        start_line: Some(line),
        start_col: Some(1),
        end_line: Some(line),
        end_col: Some(label.len().max(1) as u32),
    }
}

fn tauri_command_member_edge(file_id: NodeId, command_node_id: NodeId, line: u32) -> Edge {
    Edge {
        id: EdgeId(generate_edge_id(
            file_id.0,
            command_node_id.0,
            EdgeKind::MEMBER,
        )),
        source: file_id,
        target: command_node_id,
        kind: EdgeKind::MEMBER,
        file_node_id: Some(file_id),
        line: Some(line),
        certainty: Some(ResolutionCertainty::Certain),
        confidence: Some(0.95),
        ..Default::default()
    }
}

fn tauri_command_invoke_edge(
    file_id: NodeId,
    source_id: NodeId,
    command_node_id: NodeId,
    invocation: &TauriCommandInvocation,
    flags: IndexFeatureFlags,
) -> Edge {
    let mut edge = Edge {
        id: EdgeId(0),
        source: source_id,
        target: command_node_id,
        kind: EdgeKind::CALL,
        file_node_id: Some(file_id),
        line: Some(invocation.line),
        resolved_target: Some(command_node_id),
        certainty: Some(ResolutionCertainty::Uncertain),
        confidence: Some(0.45),
        ..Default::default()
    };
    ensure_callsite_identity(&mut edge, Some(invocation.col));
    edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
    edge
}

fn payload_collection_canonical_id(slug: &str) -> String {
    format!("payload:collection:{}", slug.trim())
}

fn payload_collection_label(slug: &str) -> String {
    format!(
        "payload collection {} (collection; confidence=heuristic)",
        slug.trim()
    )
}

fn payload_collection_node(file_id: NodeId, slug: &str, line: u32, col: u32) -> Node {
    let canonical_id = payload_collection_canonical_id(slug);
    let label = payload_collection_label(slug);
    Node {
        id: NodeId(generate_id(&canonical_id)),
        kind: NodeKind::CONSTANT,
        serialized_name: label.clone(),
        qualified_name: Some(format!("framework::payload::collection::{}", slug.trim())),
        canonical_id: Some(canonical_id),
        file_node_id: Some(file_id),
        start_line: Some(line),
        start_col: Some(col),
        end_line: Some(line),
        end_col: Some(col.saturating_add(slug.len() as u32)),
    }
}

fn payload_collection_member_edge(file_id: NodeId, collection_node_id: NodeId, line: u32) -> Edge {
    Edge {
        id: EdgeId(generate_edge_id(
            file_id.0,
            collection_node_id.0,
            EdgeKind::MEMBER,
        )),
        source: file_id,
        target: collection_node_id,
        kind: EdgeKind::MEMBER,
        file_node_id: Some(file_id),
        line: Some(line),
        certainty: Some(ResolutionCertainty::Probable),
        confidence: Some(0.70),
        ..Default::default()
    }
}

fn payload_collection_occurrence(
    file_id: NodeId,
    collection_node_id: NodeId,
    slug: &str,
    line: u32,
    col: u32,
    kind: OccurrenceKind,
) -> Occurrence {
    Occurrence {
        element_id: collection_node_id.0,
        kind,
        location: SourceLocation {
            file_node_id: file_id,
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col.saturating_add(slug.len() as u32),
        },
    }
}

struct FrameworkRouteSinks<'a> {
    unique_nodes: &'a mut HashMap<NodeId, Node>,
    result_edges: &'a mut Vec<Edge>,
    result_occurrences: &'a mut Vec<Occurrence>,
    component_access_by_node_id: &'a mut HashMap<NodeId, AccessKind>,
    edge_keys: &'a mut HashSet<EdgeDedupKey>,
    callsite_ordinals: &'a mut HashMap<(NodeId, Option<u32>), u32>,
}

fn append_framework_routes(
    path: &Path,
    language_config: &LanguageConfig,
    tree: &Tree,
    source: &str,
    file_id: NodeId,
    flags: IndexFeatureFlags,
    sinks: &mut FrameworkRouteSinks<'_>,
) -> Result<()> {
    let mut routes = collect_framework_routes(path, language_config.language_name, source)
        .into_iter()
        .map(|route| route.with_extraction_provenance("ast_indexed"))
        .collect::<Vec<_>>();

    if language_config.language_name == "python" {
        let fastapi_timeline = framework_routes::build_fastapi_binding_timeline(tree, source);
        let lexical_fastapi = routes
            .extract_if(.., |route| route.framework == "fastapi")
            .collect::<Vec<_>>();
        let parser_routes = framework_routes::collect_python_fastapi_routes_with_timeline(
            &language_config.language,
            tree,
            source,
            &fastapi_timeline,
        )?;
        let parser_keys = parser_routes
            .iter()
            .map(|route| (route.method.clone(), route.path.clone()))
            .collect::<HashSet<_>>();
        routes.extend(parser_routes);

        if tree.root_node().has_error() {
            routes.extend(
                lexical_fastapi
                    .into_iter()
                    .filter(|route| {
                        !parser_keys.contains(&(route.method.clone(), route.path.clone()))
                            && framework_routes::allow_python_fastapi_lexical_fallback(
                                tree,
                                source,
                                route,
                                &fastapi_timeline,
                            )
                    })
                    .map(|route| {
                        route
                            .with_confidence("heuristic")
                            .with_claim_evidence("lexical_fallback", "structural")
                    }),
            );
        }
    }

    if matches!(language_config.language_name, "javascript" | "typescript") {
        let framework_timeline =
            framework_routes::build_javascript_framework_timeline(tree, source);
        let lexical_express = routes
            .extract_if(.., |route| route.framework == "express")
            .collect::<Vec<_>>();
        let lexical_fastify = routes
            .extract_if(.., |route| route.framework == "fastify")
            .collect::<Vec<_>>();
        let dialect = match path.extension().and_then(|extension| extension.to_str()) {
            Some(extension) if extension.eq_ignore_ascii_case("tsx") => {
                framework_routes::JavaScriptDialect::Tsx
            }
            _ if language_config.language_name == "typescript" => {
                framework_routes::JavaScriptDialect::TypeScript
            }
            _ => framework_routes::JavaScriptDialect::JavaScript,
        };
        let parser_routes = framework_routes::collect_javascript_express_routes_with_timeline(
            &language_config.language,
            dialect,
            tree,
            source,
            &framework_timeline,
        )?;
        let parser_keys = parser_routes
            .iter()
            .map(|route| (route.method.clone(), route.path.clone()))
            .collect::<HashSet<_>>();
        routes.extend(parser_routes);

        if tree.root_node().has_error() {
            routes.extend(
                lexical_express
                    .into_iter()
                    .filter(|route| {
                        !parser_keys.contains(&(route.method.clone(), route.path.clone()))
                            && framework_routes::allow_javascript_express_lexical_fallback(
                                tree,
                                source,
                                route,
                                &framework_timeline,
                            )
                    })
                    .map(|route| {
                        route
                            .with_confidence("heuristic")
                            .with_claim_evidence("lexical_fallback", "structural")
                    }),
            );
        }

        let parser_routes = framework_routes::collect_javascript_fastify_routes_with_timeline(
            &language_config.language,
            dialect,
            tree,
            source,
            &framework_timeline,
        )?;
        let parser_keys = parser_routes
            .iter()
            .map(|route| (route.method.clone(), route.path.clone()))
            .collect::<HashSet<_>>();
        routes.extend(parser_routes);

        if tree.root_node().has_error() {
            routes.extend(
                lexical_fastify
                    .into_iter()
                    .filter(|route| {
                        !parser_keys.contains(&(route.method.clone(), route.path.clone()))
                            && framework_routes::allow_javascript_fastify_lexical_fallback(
                                tree,
                                source,
                                route,
                                &framework_timeline,
                            )
                    })
                    .map(|route| {
                        route
                            .with_confidence("heuristic")
                            .with_claim_evidence("lexical_fallback", "structural")
                    }),
            );
        }
    }

    for route in routes {
        let route_node = framework_route_node(file_id, &route);
        let route_node_id = route_node.id;
        sinks
            .unique_nodes
            .entry(route_node_id)
            .or_insert(route_node);
        sinks
            .component_access_by_node_id
            .insert(route_node_id, AccessKind::Public);

        let mut member_edge = framework_route_member_edge(file_id, route_node_id, &route);
        if sinks.edge_keys.insert(edge_dedup_key(&member_edge, flags)) {
            member_edge.id = EdgeId(generate_edge_id_for_edge(&member_edge, flags));
            sinks.result_edges.push(member_edge);
        }
        sinks
            .result_occurrences
            .push(framework_route_occurrence(file_id, route_node_id, &route));

        let Some(handler_name) = route.handler.as_deref() else {
            continue;
        };
        let Some(handler_id) =
            find_framework_route_handler(sinks.unique_nodes, handler_name, file_id, route.line)
        else {
            continue;
        };
        if handler_id == route_node_id {
            continue;
        }
        let mut call_edge = Edge {
            id: EdgeId(0),
            source: route_node_id,
            target: handler_id,
            kind: EdgeKind::CALL,
            file_node_id: Some(file_id),
            line: Some(route.line),
            certainty: Some(ResolutionCertainty::Probable),
            confidence: Some(0.65),
            ..Default::default()
        };
        let next = sinks
            .callsite_ordinals
            .entry((handler_id, call_edge.line))
            .or_insert(0);
        *next = next.saturating_add(1);
        ensure_callsite_identity(&mut call_edge, Some(*next));
        if !sinks.edge_keys.insert(edge_dedup_key(&call_edge, flags)) {
            continue;
        }
        call_edge.id = EdgeId(generate_edge_id_for_edge(&call_edge, flags));
        sinks.result_edges.push(call_edge);
    }
    Ok(())
}

fn find_framework_route_handler(
    nodes: &HashMap<NodeId, Node>,
    handler_name: &str,
    file_id: NodeId,
    route_line: u32,
) -> Option<NodeId> {
    let terminal = handler_name
        .rsplit(['.', ':', '#', '@'])
        .next()
        .unwrap_or(handler_name)
        .trim()
        .trim_end_matches(']')
        .trim_end_matches('}');
    if terminal.is_empty() {
        return None;
    }
    let matches = nodes
        .values()
        .filter(|node| is_callable_kind(node.kind) && node_matches_name(node, terminal))
        .collect::<Vec<_>>();
    let mut same_file_matches = matches
        .iter()
        .copied()
        .filter(|node| node.file_node_id == Some(file_id))
        .map(|node| {
            let start_line = node.start_line.unwrap_or(u32::MAX);
            (
                node.id,
                (
                    start_line.abs_diff(route_line),
                    start_line,
                    node_span_width(node),
                ),
            )
        })
        .collect::<Vec<_>>();

    if same_file_matches.is_empty() {
        return match matches.as_slice() {
            [only] => Some(only.id),
            _ => None,
        };
    }

    same_file_matches.sort_by_key(|(node_id, score)| (score.0, score.1, score.2, *node_id));
    match same_file_matches.as_slice() {
        [first, second, ..] if first.1 == second.1 => None,
        [first, ..] => Some(first.0),
        [] => None,
    }
}

fn find_registered_tauri_command_function(
    nodes: &HashMap<NodeId, Node>,
    command_name: &str,
) -> Option<NodeId> {
    let mut matches = nodes
        .values()
        .filter(|node| {
            is_callable_kind(node.kind)
                && !node
                    .canonical_id
                    .as_deref()
                    .is_some_and(|value| value.starts_with("tauri:command:"))
                && node_matches_name(node, command_name)
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|node| {
        (
            node.start_line.unwrap_or(u32::MAX),
            node_span_width(node),
            node.id,
        )
    });
    matches.first().map(|node| node.id)
}

fn non_impl_anchor_canonical_predicate() -> &'static str {
    "AND (canonical_id IS NULL OR canonical_id NOT LIKE 'impl_anchor:%')"
}

struct SchemaEndpointEdgeSinks<'a> {
    unique_nodes: &'a mut HashMap<NodeId, Node>,
    result_edges: &'a mut Vec<Edge>,
    edge_keys: &'a mut HashSet<EdgeDedupKey>,
    callsite_ordinals: &'a mut HashMap<(NodeId, Option<u32>), u32>,
}

fn append_schema_endpoint_call_edges(
    language_name: &str,
    source: &str,
    file_id: NodeId,
    flags: IndexFeatureFlags,
    sinks: &mut SchemaEndpointEdgeSinks<'_>,
) {
    if !matches!(
        language_name,
        "javascript" | "typescript" | "python" | "rust" | "java" | "go"
    ) {
        return;
    }

    for call in collect_api_endpoint_calls(source) {
        let target = schema_endpoint_node_id(&call.method, &call.path);
        let target_label = schema_endpoint_label(&call.method, &call.path);
        let source_id =
            enclosing_callable_node_id(sinks.unique_nodes, call.line).unwrap_or(file_id);
        sinks.unique_nodes.entry(target).or_insert_with(|| Node {
            id: target,
            kind: NodeKind::FUNCTION,
            serialized_name: target_label.clone(),
            qualified_name: Some(format!("schema_reference::{target_label}")),
            canonical_id: Some(format!("openapi:endpoint:{target_label}")),
            file_node_id: Some(file_id),
            start_line: Some(call.line),
            start_col: Some(call.col),
            end_line: Some(call.line),
            end_col: Some(call.col.saturating_add(call.path.len() as u32)),
        });

        if source_id == target {
            continue;
        }
        let mut edge = Edge {
            id: EdgeId(0),
            source: source_id,
            target,
            kind: EdgeKind::CALL,
            file_node_id: Some(file_id),
            line: Some(call.line),
            certainty: Some(ResolutionCertainty::Uncertain),
            confidence: Some(0.45),
            ..Default::default()
        };
        let next = sinks
            .callsite_ordinals
            .entry((target, edge.line))
            .or_insert(0);
        *next = next.saturating_add(1);
        ensure_callsite_identity(&mut edge, Some(call.col.saturating_add(*next)));
        if !sinks.edge_keys.insert(edge_dedup_key(&edge, flags)) {
            continue;
        }
        edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
        sinks.result_edges.push(edge);
    }
}

struct FrameworkSymbolSinks<'a> {
    unique_nodes: &'a mut HashMap<NodeId, Node>,
    result_edges: &'a mut Vec<Edge>,
    result_occurrences: &'a mut Vec<Occurrence>,
    component_access_by_node_id: &'a mut HashMap<NodeId, AccessKind>,
    edge_keys: &'a mut HashSet<EdgeDedupKey>,
    callsite_ordinals: &'a mut HashMap<(NodeId, Option<u32>), u32>,
}

fn append_tauri_command_registrations(
    language_name: &str,
    source: &str,
    file_id: NodeId,
    flags: IndexFeatureFlags,
    sinks: &mut FrameworkSymbolSinks<'_>,
) {
    if language_name != "rust" {
        return;
    }

    for registration in collect_tauri_command_registrations(source) {
        let command_node = tauri_command_node(file_id, &registration.command, registration.line);
        let command_node_id = command_node.id;
        sinks
            .unique_nodes
            .entry(command_node_id)
            .or_insert(command_node);
        sinks
            .component_access_by_node_id
            .insert(command_node_id, AccessKind::Public);
        sinks.result_occurrences.push(payload_collection_occurrence(
            file_id,
            command_node_id,
            &registration.command,
            registration.line,
            1,
            OccurrenceKind::DEFINITION,
        ));

        let mut member_edge =
            tauri_command_member_edge(file_id, command_node_id, registration.line);
        if sinks.edge_keys.insert(edge_dedup_key(&member_edge, flags)) {
            member_edge.id = EdgeId(generate_edge_id_for_edge(&member_edge, flags));
            sinks.result_edges.push(member_edge);
        }

        let Some(function_id) =
            find_registered_tauri_command_function(sinks.unique_nodes, &registration.command)
        else {
            continue;
        };
        if function_id == command_node_id {
            continue;
        }
        let mut call_edge = Edge {
            id: EdgeId(0),
            source: command_node_id,
            target: function_id,
            kind: EdgeKind::CALL,
            file_node_id: Some(file_id),
            line: Some(registration.line),
            certainty: Some(ResolutionCertainty::Probable),
            confidence: Some(0.70),
            ..Default::default()
        };
        let next = sinks
            .callsite_ordinals
            .entry((function_id, call_edge.line))
            .or_insert(0);
        *next = next.saturating_add(1);
        ensure_callsite_identity(&mut call_edge, Some(*next));
        if !sinks.edge_keys.insert(edge_dedup_key(&call_edge, flags)) {
            continue;
        }
        call_edge.id = EdgeId(generate_edge_id_for_edge(&call_edge, flags));
        sinks.result_edges.push(call_edge);
    }
}

fn append_payload_collection_symbols(
    language_name: &str,
    source: &str,
    file_id: NodeId,
    flags: IndexFeatureFlags,
    sinks: &mut FrameworkSymbolSinks<'_>,
) {
    if !matches!(language_name, "javascript" | "typescript" | "tsx") {
        return;
    }

    for registration in collect_payload_collection_registrations(source) {
        let collection_node =
            payload_collection_node(file_id, &registration.slug, registration.line, 1);
        let collection_node_id = collection_node.id;
        sinks
            .unique_nodes
            .entry(collection_node_id)
            .or_insert(collection_node);
        sinks
            .component_access_by_node_id
            .insert(collection_node_id, AccessKind::Public);
        sinks.result_occurrences.push(payload_collection_occurrence(
            file_id,
            collection_node_id,
            &registration.slug,
            registration.line,
            1,
            OccurrenceKind::DEFINITION,
        ));

        let mut member_edge =
            payload_collection_member_edge(file_id, collection_node_id, registration.line);
        if sinks.edge_keys.insert(edge_dedup_key(&member_edge, flags)) {
            member_edge.id = EdgeId(generate_edge_id_for_edge(&member_edge, flags));
            sinks.result_edges.push(member_edge);
        }
    }

    for usage in collect_payload_collection_usages(source) {
        let collection_node = payload_collection_node(file_id, &usage.slug, usage.line, usage.col);
        let collection_node_id = collection_node.id;
        sinks
            .unique_nodes
            .entry(collection_node_id)
            .or_insert(collection_node);
        sinks.result_occurrences.push(payload_collection_occurrence(
            file_id,
            collection_node_id,
            &usage.slug,
            usage.line,
            usage.col,
            OccurrenceKind::REFERENCE,
        ));

        let source_id =
            enclosing_callable_node_id(sinks.unique_nodes, usage.line).unwrap_or(file_id);
        if source_id == collection_node_id {
            continue;
        }
        let mut edge = Edge {
            id: EdgeId(0),
            source: source_id,
            target: collection_node_id,
            kind: EdgeKind::USAGE,
            file_node_id: Some(file_id),
            line: Some(usage.line),
            certainty: Some(ResolutionCertainty::Probable),
            confidence: Some(0.65),
            callsite_identity: Some(format!(
                "payload:{}:{}:{}:{}",
                usage.operation, usage.slug, usage.line, usage.col
            )),
            ..Default::default()
        };
        if edge.kind == EdgeKind::CALL && !flags.legacy_edge_identity {
            let next = sinks
                .callsite_ordinals
                .entry((collection_node_id, edge.line))
                .or_insert(0);
            *next = next.saturating_add(1);
            ensure_callsite_identity(&mut edge, Some(usage.col.saturating_add(*next)));
        }
        if !sinks.edge_keys.insert(edge_dedup_key(&edge, flags)) {
            continue;
        }
        edge.id = EdgeId(generate_edge_id_for_edge(&edge, flags));
        sinks.result_edges.push(edge);
    }
}

#[derive(Debug, Clone)]
struct ApiEndpointCall {
    method: String,
    path: String,
    line: u32,
    col: u32,
}

fn collect_api_endpoint_calls(source: &str) -> Vec<ApiEndpointCall> {
    let mut calls = Vec::new();
    for (line_index, line) in source.lines().enumerate() {
        for (literal, col) in quoted_string_literals(line) {
            if !is_api_path_literal(&literal) || !is_api_endpoint_call_context(line, col) {
                continue;
            }
            calls.push(ApiEndpointCall {
                method: infer_http_method(line),
                path: normalize_api_path(&literal),
                line: line_index as u32 + 1,
                col,
            });
        }
    }
    calls
}

fn quoted_string_literals(line: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if !matches!(ch, '"' | '\'' | '`') {
            continue;
        }
        let quote = ch;
        let mut value = String::new();
        let mut escaped = false;
        for (_, next) in chars.by_ref() {
            if escaped {
                value.push(next);
                escaped = false;
                continue;
            }
            if next == '\\' {
                escaped = true;
                continue;
            }
            if next == quote {
                break;
            }
            value.push(next);
        }
        out.push((value, index as u32 + 1));
    }
    out
}

fn code_before_line_comment(line: &str) -> &str {
    let mut chars = line.char_indices().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    while let Some((index, ch)) = chars.next() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }

        if matches!(ch, '"' | '\'' | '`') {
            quote = Some(ch);
            continue;
        }
        if ch == '/'
            && let Some((_, next)) = chars.peek()
            && *next == '/'
        {
            return &line[..index];
        }
    }
    line
}

fn code_before_hash_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }

        if matches!(ch, '"' | '\'') {
            quote = Some(ch);
            continue;
        }
        if ch == '#' {
            return &line[..index];
        }
    }
    line
}

fn quoted_value_after_key(line: &str, key: &str) -> Option<(String, u32)> {
    let (_key_start, colon_index) = object_key_colon_index(line, key)?;
    let value_tail = &line[colon_index + 1..];
    let base = colon_index + 1;
    quoted_string_literals(value_tail)
        .into_iter()
        .next()
        .map(|(value, col)| (value, base as u32 + col))
}

fn object_key_colon_index(line: &str, key: &str) -> Option<(usize, usize)> {
    let lower = line.to_ascii_lowercase();
    let key_lower = key.to_ascii_lowercase();
    for (key_start, _) in lower.match_indices(&key_lower) {
        if key_start > 0
            && lower[..key_start]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            continue;
        }

        let mut cursor = key_start + key_lower.len();
        if lower[cursor..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            continue;
        }
        cursor = skip_ascii_whitespace(&lower, cursor);
        if lower[cursor..].starts_with(':') {
            return Some((key_start, cursor));
        }
    }
    None
}

fn quoted_value_after_key_across_lines(
    lines: &[&str],
    line_index: usize,
    key: &str,
) -> Option<(String, u32, u32)> {
    let line = lines.get(line_index)?;
    if let Some((value, col)) = quoted_value_after_key(line, key) {
        return Some((value, line_index as u32 + 1, col));
    }

    let code = code_before_line_comment(line);
    object_key_colon_index(code, key)?;

    for lookahead in 1..=2 {
        let next_index = line_index + lookahead;
        let Some(next_line) = lines.get(next_index) else {
            break;
        };
        let next_code = code_before_line_comment(next_line).trim();
        if next_code.is_empty() {
            continue;
        }
        return quoted_string_literals(next_line)
            .into_iter()
            .next()
            .map(|(value, col)| (value, next_index as u32 + 1, col));
    }
    None
}

fn structural_delta(line: &str, open: char, close: char) -> i32 {
    let mut delta = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in code_before_line_comment(line).chars() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }

        if matches!(ch, '"' | '\'' | '`') {
            quote = Some(ch);
        } else if ch == open {
            delta += 1;
        } else if ch == close {
            delta -= 1;
        }
    }
    delta
}

fn collect_tauri_command_invocations(source: &str) -> Vec<TauriCommandInvocation> {
    let mut invocations = Vec::new();
    let mut seen = HashSet::new();
    let mut pending_first_arg_lines = 0u8;
    for (line_index, line) in source.lines().enumerate() {
        let mut accepted = false;
        for (literal, col) in quoted_string_literals(line) {
            let pending_literal =
                pending_first_arg_lines > 0 && is_pending_first_arg_literal(line, col);
            if literal.trim().is_empty()
                || (!is_tauri_invoke_context(line, col) && !pending_literal)
            {
                continue;
            }
            let command = literal.trim().to_string();
            if seen.insert((command.clone(), line_index as u32 + 1, col)) {
                invocations.push(TauriCommandInvocation {
                    command,
                    line: line_index as u32 + 1,
                    col,
                });
            }
            pending_first_arg_lines = 0;
            accepted = true;
            break;
        }
        if accepted {
            continue;
        }

        if tauri_invoke_waits_for_first_arg(line) {
            pending_first_arg_lines = 8;
        } else if pending_first_arg_lines > 0 {
            let code = code_before_line_comment(line).trim();
            if code.is_empty() {
                pending_first_arg_lines = pending_first_arg_lines.saturating_sub(1);
            } else {
                pending_first_arg_lines = 0;
            }
        }
    }
    invocations
}

fn is_tauri_invoke_context(line: &str, literal_col: u32) -> bool {
    let literal_start = literal_col.saturating_sub(1) as usize;
    let Some(before_literal) = line.get(..literal_start) else {
        return false;
    };
    if has_line_comment_before_literal(before_literal) {
        return false;
    }
    let Some(open_paren) = tauri_invoke_open_paren(before_literal) else {
        return false;
    };
    before_literal[open_paren + 1..].trim().is_empty()
}

fn is_pending_first_arg_literal(line: &str, literal_col: u32) -> bool {
    let literal_start = literal_col.saturating_sub(1) as usize;
    let Some(before_literal) = line.get(..literal_start) else {
        return false;
    };
    !has_line_comment_before_literal(before_literal) && before_literal.trim().is_empty()
}

fn tauri_invoke_waits_for_first_arg(line: &str) -> bool {
    let code = code_before_line_comment(line);
    let Some(open_paren) = tauri_invoke_open_paren(code) else {
        return false;
    };
    code[open_paren + 1..].trim().is_empty()
}

fn tauri_invoke_open_paren(text: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    for (index, _) in lower.match_indices("invoke") {
        if index > 0
            && lower[..index]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            continue;
        }

        let mut cursor = index + "invoke".len();
        cursor = skip_ascii_whitespace(&lower, cursor);
        if lower[cursor..].starts_with("::") {
            cursor += 2;
            cursor = skip_ascii_whitespace(&lower, cursor);
        }
        if lower[cursor..].starts_with('<') {
            let Some(generic_end) = lower[cursor..].find('>') else {
                continue;
            };
            cursor += generic_end + 1;
            cursor = skip_ascii_whitespace(&lower, cursor);
        }
        if lower[cursor..].starts_with('(') {
            return Some(cursor);
        }
    }
    None
}

fn skip_ascii_whitespace(text: &str, mut cursor: usize) -> usize {
    while text
        .as_bytes()
        .get(cursor)
        .is_some_and(u8::is_ascii_whitespace)
    {
        cursor += 1;
    }
    cursor
}

fn collect_tauri_command_registrations(source: &str) -> Vec<TauriCommandRegistration> {
    let mut registrations = Vec::new();
    let mut seen = HashSet::new();
    let mut pending_attr_line: Option<u32> = None;
    let mut generate_handler_buffer: Option<(u32, String)> = None;

    for (line_index, line) in source.lines().enumerate() {
        let line_number = line_index as u32 + 1;
        let trimmed = code_before_line_comment(line).trim();

        if let Some((start_line, buffer)) = generate_handler_buffer.as_mut() {
            if !trimmed.is_empty() {
                buffer.push(' ');
                buffer.push_str(trimmed);
            }
            if trimmed.contains(']') {
                let commands = tauri_generate_handler_commands(buffer);
                for command in commands {
                    if seen.insert(command.clone()) {
                        registrations.push(TauriCommandRegistration {
                            command,
                            line: *start_line,
                        });
                    }
                }
                generate_handler_buffer = None;
            }
            continue;
        }

        if trimmed.contains("tauri::generate_handler!") {
            let mut buffer = trimmed.to_string();
            if buffer.contains(']') {
                for command in tauri_generate_handler_commands(&buffer) {
                    if seen.insert(command.clone()) {
                        registrations.push(TauriCommandRegistration {
                            command,
                            line: line_number,
                        });
                    }
                }
            } else {
                generate_handler_buffer = Some((line_number, std::mem::take(&mut buffer)));
            }
        }

        if trimmed.contains("#[tauri::command") {
            pending_attr_line = Some(line_number);
            continue;
        }

        let Some(attr_line) = pending_attr_line else {
            continue;
        };
        if trimmed.is_empty() || trimmed.starts_with("#[") || trimmed.starts_with("//") {
            continue;
        }
        if let Some(command) = rust_function_name(trimmed)
            && seen.insert(command.clone())
        {
            registrations.push(TauriCommandRegistration {
                command,
                line: attr_line,
            });
        }
        pending_attr_line = None;
    }

    registrations
}

fn tauri_generate_handler_commands(buffer: &str) -> Vec<String> {
    let inside = buffer
        .split_once('[')
        .and_then(|(_, tail)| tail.rsplit_once(']').map(|(inside, _)| inside))
        .unwrap_or_default();
    inside
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':'))
        .filter_map(|token| {
            let name = token.rsplit("::").next().unwrap_or(token).trim();
            if name.is_empty()
                || matches!(
                    name,
                    "tauri" | "generate_handler" | "Builder" | "invoke_handler"
                )
            {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect()
}

fn rust_function_name(line: &str) -> Option<String> {
    let (_, tail) = line.split_once("fn ")?;
    let name = tail
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect::<String>();
    (!name.is_empty()).then_some(name)
}

fn collect_payload_collection_registrations(source: &str) -> Vec<PayloadCollectionRegistration> {
    let lower_source = source.to_ascii_lowercase();
    if !lower_source.contains("collectionconfig") && !lower_source.contains("payload") {
        return Vec::new();
    }

    let mut registrations = Vec::new();
    let mut seen = HashSet::new();
    let lines = source.lines().collect::<Vec<_>>();
    let mut block_start = 0usize;
    let mut block_lines: Vec<&str> = Vec::new();
    let mut brace_depth = 0i32;
    let mut saw_open_brace = false;

    for (line_index, line) in lines.iter().enumerate() {
        let code = code_before_line_comment(line);
        let starts_candidate =
            block_lines.is_empty() && starts_payload_collection_config_block(code);
        if starts_candidate {
            block_start = line_index;
            brace_depth = 0;
            saw_open_brace = false;
        }

        if starts_candidate || !block_lines.is_empty() {
            block_lines.push(line);
            let delta = structural_delta(code, '{', '}');
            if delta > 0 {
                saw_open_brace = true;
            }
            brace_depth += delta;

            if saw_open_brace && brace_depth <= 0 {
                let block_text = block_lines.join("\n");
                if block_text.to_ascii_lowercase().contains("collectionconfig")
                    && let Some((slug, line, _col)) =
                        quoted_value_after_key_in_block(&block_lines, block_start, "slug")
                    && !slug.trim().is_empty()
                    && seen.insert(slug.clone())
                {
                    registrations.push(PayloadCollectionRegistration { slug, line });
                }
                block_lines.clear();
                brace_depth = 0;
                saw_open_brace = false;
            }
        }
    }
    registrations
}

fn collect_payload_collection_usages(source: &str) -> Vec<PayloadCollectionUsage> {
    let lower_source = source.to_ascii_lowercase();
    if !lower_source.contains("payload.")
        && !lower_source.contains("req.payload")
        && !lower_source.contains("getpayload")
    {
        return Vec::new();
    }

    let mut usages = Vec::new();
    let mut seen = HashSet::new();
    let lines = source.lines().collect::<Vec<_>>();
    let mut pending_payload_call: Option<(&'static str, u8)> = None;
    for (line_index, line) in lines.iter().enumerate() {
        let code = code_before_line_comment(line);
        if let Some(operation) = payload_collection_call_operation(code) {
            pending_payload_call = Some((operation, 16));
        }

        let trimmed = code.trim();
        let Some((operation, remaining)) = pending_payload_call else {
            continue;
        };
        if !trimmed.contains("collection") {
            let updated = update_pending_payload_call(remaining, code);
            pending_payload_call = if updated == 0 {
                None
            } else {
                Some((operation, updated))
            };
            continue;
        }

        let Some((slug, value_line, col)) =
            quoted_value_after_key_across_lines(&lines, line_index, "collection")
        else {
            let updated = update_pending_payload_call(remaining, code);
            pending_payload_call = if updated == 0 {
                None
            } else {
                Some((operation, updated))
            };
            continue;
        };
        if slug.trim().is_empty() {
            let updated = update_pending_payload_call(remaining, code);
            pending_payload_call = if updated == 0 {
                None
            } else {
                Some((operation, updated))
            };
            continue;
        }
        if seen.insert((slug.clone(), operation.to_string(), value_line, col)) {
            usages.push(PayloadCollectionUsage {
                slug,
                operation: operation.to_string(),
                line: value_line,
                col,
            });
        }
        let updated = update_pending_payload_call(remaining, code);
        pending_payload_call = if updated == 0 {
            None
        } else {
            Some((operation, updated))
        };
    }
    usages
}

fn payload_collection_call_operation(line: &str) -> Option<&'static str> {
    let compact = compact_lowercase(line);
    [
        ("payload.find(", "find"),
        ("payload.findbyid(", "find_by_id"),
        ("payload.create(", "create"),
        ("payload.update(", "update"),
        ("payload.delete(", "delete"),
        ("payload.count(", "count"),
        ("payload.restoreversion(", "restore_version"),
        ("payload.deleteversion(", "delete_version"),
        ("req.payload.find(", "find"),
        ("req.payload.findbyid(", "find_by_id"),
        ("req.payload.create(", "create"),
        ("req.payload.update(", "update"),
        ("req.payload.delete(", "delete"),
        ("req.payload.count(", "count"),
    ]
    .iter()
    .find_map(|(needle, operation)| compact.contains(needle).then_some(*operation))
}

fn starts_payload_collection_config_block(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let trimmed = lower.trim_start();
    let collection_config_declaration = lower.contains("collectionconfig")
        && !trimmed.starts_with("import ")
        && !trimmed.starts_with("type ");
    collection_config_declaration
        || (lower.contains("export const ") && lower.contains('{'))
        || (lower.trim_start().starts_with("const ") && lower.contains('{'))
}

fn quoted_value_after_key_in_block(
    block_lines: &[&str],
    block_start: usize,
    key: &str,
) -> Option<(String, u32, u32)> {
    for index in 0..block_lines.len() {
        if !code_before_line_comment(block_lines[index]).contains(key) {
            continue;
        }
        if let Some((value, line, col)) =
            quoted_value_after_key_across_lines(block_lines, index, key)
        {
            return Some((value, block_start as u32 + line, col));
        }
    }
    None
}

fn update_pending_payload_call(remaining: u8, line: &str) -> u8 {
    if remaining == 0 {
        return 0;
    }
    let delta = structural_delta(line, '(', ')');
    let code = code_before_line_comment(line);
    if delta < 0 || code.contains(");") || (delta <= 0 && code.contains(')')) {
        0
    } else {
        remaining.saturating_sub(1)
    }
}

fn is_api_path_literal(value: &str) -> bool {
    let path = value.trim();
    path.starts_with('/')
        && path.len() > 1
        && !path.starts_with("//")
        && !path.chars().any(char::is_whitespace)
        && path.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn is_api_endpoint_call_context(line: &str, literal_col: u32) -> bool {
    let literal_start = literal_col.saturating_sub(1) as usize;
    let Some(before_literal) = line.get(..literal_start) else {
        return false;
    };
    let trimmed_before = before_literal.trim_start();
    if trimmed_before.starts_with("//") || trimmed_before.starts_with('#') {
        return false;
    }
    if has_line_comment_before_literal(before_literal) {
        return false;
    }

    let compact_before = compact_lowercase(before_literal);
    if compact_before.contains("fetch(") {
        return true;
    }

    let methods = ["delete", "patch", "post", "put", "head", "options", "get"];
    methods.iter().any(|method| {
        let dot_call = format!(".{method}(");
        let path_call = format!("::{method}(");
        (compact_before.ends_with(&dot_call) || compact_before.ends_with(&path_call))
            && !is_server_route_registration_context(&compact_before, method)
    })
}

fn is_server_route_registration_context(compact_before: &str, method: &str) -> bool {
    let route_call = format!(".{method}(");
    let Some(receiver) = compact_before.strip_suffix(&route_call) else {
        return false;
    };
    let receiver = receiver
        .rsplit(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '.'))
        .next()
        .unwrap_or(receiver)
        .rsplit('.')
        .next()
        .unwrap_or(receiver);
    matches!(
        receiver,
        "app" | "router" | "route" | "server" | "fastify" | "hono"
    )
}

fn has_line_comment_before_literal(value: &str) -> bool {
    let mut chars = value.char_indices().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    while let Some((idx, ch)) = chars.next() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '/' if chars.peek().is_some_and(|(_, next)| *next == '/') => return true,
            '#' => {
                let starts_comment = value[..idx].trim().is_empty()
                    || value[..idx]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_whitespace);
                if starts_comment {
                    return true;
                }
            }
            _ => {}
        }
    }

    false
}

fn compact_lowercase(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn infer_http_method(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    for method in ["delete", "patch", "post", "put", "head", "options", "get"] {
        if lower.contains(&format!(".{method}("))
            || lower.contains(&format!("method: \"{method}\""))
            || lower.contains(&format!("method: '{method}'"))
            || lower.contains(&format!("method = \"{method}\""))
            || lower.contains(&format!("method = '{method}'"))
            || lower.contains(&format!("\"{method}\""))
            || lower.contains(&format!("'{method}'"))
        {
            return method.to_ascii_uppercase();
        }
    }
    "GET".to_string()
}

fn enclosing_callable_node_id(nodes: &HashMap<NodeId, Node>, line: u32) -> Option<NodeId> {
    nodes
        .values()
        .filter(|node| {
            matches!(
                node.kind,
                NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
            ) && !node
                .canonical_id
                .as_deref()
                .is_some_and(|value| value.starts_with("openapi:endpoint:"))
                && node.start_line.unwrap_or(u32::MAX) <= line
                && node.end_line.unwrap_or(0) >= line
        })
        .min_by_key(|node| {
            node.end_line
                .unwrap_or(line)
                .saturating_sub(node.start_line.unwrap_or(line))
        })
        .map(|node| node.id)
}

/// Index one file with an already selected parser-backed language config.
///
/// This function expects the caller to provide the source and matching
/// `LanguageConfig`. It emits parser-backed nodes, edges, occurrences, and
/// callable projection state for that file, plus delegated structural CSS from
/// supported template files.
pub fn index_file(
    path: &Path,
    source: &str,
    language_config: &LanguageConfig,
    compilation_info: Option<compilation_database::CompilationInfo>,
    symbol_table: Option<Arc<SymbolTable>>,
) -> Result<IndexResult> {
    let source_sha256 = source_content_hash(source.as_bytes());
    index_file_with_resolution_inputs(
        path,
        source,
        &source_sha256,
        language_config,
        compilation_info,
        symbol_table,
    )
    .map(|(result, _, _)| result)
}

fn index_file_with_resolution_inputs(
    path: &Path,
    source: &str,
    raw_source_sha256: &str,
    language_config: &LanguageConfig,
    compilation_info: Option<compilation_database::CompilationInfo>,
    symbol_table: Option<Arc<SymbolTable>>,
) -> Result<(
    IndexResult,
    Vec<cache::CachedCallResolutionInput>,
    Option<cache::CachedResolutionFile>,
)> {
    let flags = index_feature_flags();
    let is_jsx_like_file = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("tsx") || ext.eq_ignore_ascii_case("jsx"))
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
    let declaration_span_overrides =
        collect_declaration_span_overrides(language_config.language_name, &tree, source);

    let mut variables = Variables::new();
    if let Some(info) = &compilation_info {
        // Inject compilation info into graph variables
        for (name, value) in &info.defines {
            let val = value.as_deref().unwrap_or("1");
            let _ = variables.add(name.as_str().into(), val.into());
        }
    }

    let functions = Functions::stdlib();
    let config = ExecutionConfig::new(&functions, &variables).lazy(flags.lazy_graph_execution);

    let graph = compiled_rules
        .graph_file
        .execute(&tree, source, &config, &NoCancellation)
        .map_err(|e| anyhow!("Graph execution error: {:?}", e))?;

    let mut reference_graph_nodes = HashSet::new();
    for source_ref in graph.iter_nodes() {
        for (sink_ref, edge) in graph[source_ref].iter_edges() {
            let relation = edge.attributes.iter().find_map(|(attr, value)| {
                (attr.as_str() == "kind")
                    .then(|| value.as_str().ok())
                    .flatten()
                    .and_then(edge_kind_from_str)
            });
            if relation.is_some_and(graph_relation_sink_is_reference) {
                reference_graph_nodes.insert(sink_ref);
            }
        }
    }

    let mut result_files = Vec::new();
    let mut result_nodes = Vec::new();
    let mut result_edges = Vec::new();
    let mut result_occurrences = Vec::new();

    // 0. Create file node and FileInfo
    let (file_node, file_name, file_id) = file_node_from_source(path, source);
    result_nodes.push(file_node);

    let modification_time = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(codestory_workspace::clamp_system_time_to_epoch_millis)
        .unwrap_or(0);

    result_files.push(codestory_store::FileInfo {
        id: file_id.0,
        path: path.to_path_buf(),
        language: language_config.language_name.to_string(),
        modification_time,
        indexed: true,
        complete: !tree.root_node().has_error(),
        line_count: source.lines().count() as u32,
        file_role: codestory_store::FileRole::classify_path(path),
    });

    // 1. First pass: Create nodes and a temporary mapping from GraphNodeId -> OurNodeId
    let mut graph_to_node_id = HashMap::new();
    let line_offsets = LineOffsets::new(source);
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
        let mut rust_impl_expr = false;

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
                "rust_impl_expr" => rust_impl_expr = true,
                _ => {}
            }
        }
        if canonical_role == CanonicalNodeRole::Unspecified {
            canonical_role = if reference_graph_nodes.contains(&node_id) {
                CanonicalNodeRole::Reference
            } else {
                CanonicalNodeRole::Definition
            };
        }
        let has_token_surface_edge = node_data.iter_edges().any(|(_, edge)| {
            edge.attributes
                .iter()
                .find_map(|(attr, val)| {
                    if attr.as_str() != "kind" {
                        return None;
                    }
                    val.as_str()
                        .ok()
                        .map(|kind| matches!(kind, "CALL" | "IMPORT" | "ANNOTATION_USAGE"))
                })
                .unwrap_or(false)
        });

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
                && languages::python::is_constant_name(&name_str)
            {
                kind = NodeKind::CONSTANT;
            }
            let qualified_name_override =
                if language_config.language_name == "rust" && rust_impl_expr {
                    rust_impl_expr_qualified_name(&name_str)
                } else {
                    None
                };
            let span_policy = graph_capture_span_policy(
                language_config.language_name,
                kind,
                canonical_role,
                rust_impl_expr,
                &name_str,
                has_token_surface_edge,
            );

            let mut start_line = start_row.map(|v| v + 1).unwrap_or(1);
            let mut start_col_1 = start_col.map(|v| v + 1).unwrap_or(1);
            let mut end_line_1 = end_row.map(|v| v + 1).unwrap_or(start_line);
            let mut end_col_1 = end_col.map(|v| v + 1).unwrap_or(start_col_1);
            if let Some((
                normalized_name,
                normalized_start_line,
                normalized_start_col,
                normalized_end_line,
                normalized_end_col,
            )) = normalize_graph_capture(&GraphCaptureNormalizationInput {
                language_name: language_config.language_name,
                kind,
                canonical_role,
                rust_impl_expr,
                name: &name_str,
                graph_span: GraphNodeSpan {
                    start_line,
                    start_col: start_col_1,
                    end_line: end_line_1,
                    end_col: end_col_1,
                },
                source,
                has_token_surface_edge,
            }) {
                name_str = normalized_name;
                start_line = normalized_start_line;
                start_col_1 = normalized_start_col;
                end_line_1 = normalized_end_line;
                end_col_1 = normalized_end_col;
            }
            let declaration_span_key = DeclarationSpanOverrideKey {
                kind,
                name: name_str.clone(),
                token_line: start_line,
                token_col: start_col_1,
            };
            if span_policy == NodeSpanPolicy::Definition
                && let Some(definition) =
                    tag_definitions.take(&name_str, start_line, Some(start_col_1))
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
            if span_policy == NodeSpanPolicy::Definition
                && let Some(override_span) = declaration_span_overrides.get(&declaration_span_key)
            {
                start_line = override_span.start_line;
                start_col_1 = override_span.start_col;
                end_line_1 = override_span.end_line;
                end_col_1 = override_span.end_col;
            }
            let canonical_seed = if matches!(
                canonical_role,
                CanonicalNodeRole::Definition
                    | CanonicalNodeRole::Declaration
                    | CanonicalNodeRole::ForwardDeclaration
            ) {
                format!("{}:{}:{}:{}", file_name, name_str, start_line, start_col_1)
            } else {
                format!("{}:{}:{}", file_name, name_str, start_line)
            };
            let nid = NodeId(generate_id(&canonical_seed));
            graph_to_node_id.insert(node_id, nid);
            let effective_access = if language_config.language_name == "swift"
                && matches!(
                    kind,
                    NodeKind::STRUCT
                        | NodeKind::CLASS
                        | NodeKind::ENUM
                        | NodeKind::FUNCTION
                        | NodeKind::METHOD
                )
                && matches!(
                    canonical_role,
                    CanonicalNodeRole::Definition
                        | CanonicalNodeRole::Declaration
                        | CanonicalNodeRole::ForwardDeclaration
                ) {
                Some(
                    if proof_resolution::swift_declaration_cross_module_visible_at(
                        &tree,
                        source,
                        start_line,
                        start_col_1,
                    ) {
                        AccessKind::Public
                    } else {
                        AccessKind::Default
                    },
                )
            } else {
                access_kind.or_else(|| {
                    infer_access_from_source(
                        language_config.language_name,
                        &tree,
                        source,
                        &line_offsets,
                        start_line,
                        kind,
                    )
                })
            };

            unique_nodes.insert(
                nid,
                Node {
                    id: nid,
                    kind,
                    serialized_name: name_str,
                    qualified_name: qualified_name_override,
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
            "{}:{}:{}:{}",
            file_name, definition.key.name, definition.key.start_line, definition.key.start_col
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
        canonical_role_by_node_id.insert(
            nid,
            if definition.canonical_role == CanonicalNodeRole::Unspecified {
                CanonicalNodeRole::Definition
            } else {
                definition.canonical_role
            },
        );
        if let Some(access) = definition.access {
            component_access_by_node_id.insert(nid, access);
        }
        if let Some(st) = &symbol_table {
            st.insert(nid.0, definition.kind);
        }
    }

    let runtime_import_specs = collect_runtime_import_specs(
        language_config.language_name,
        &file_name,
        &tree,
        source,
        &mut unique_nodes,
        symbol_table.as_ref(),
    );

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
            let mut callsite_marker: Option<&'static str> = None;

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
                    "call_syntax" => {
                        if let Ok(raw) = val.as_str() {
                            let raw = raw.trim();
                            // Every rule file's `call_syntax` now resolves
                            // through the registry; a syntax it does not know
                            // leaves the marker as it was.
                            callsite_marker =
                                languages::member_callsite_marker_for_call_syntax(raw)
                                    .or(callsite_marker);
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
                certainty: parser_direct_structural_certainty(kind),
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
            if let Some(marker) = callsite_marker {
                append_callsite_marker(&mut edge, marker);
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
        &mut callsite_ordinals,
    );
    append_manual_usage_edges(
        language_config.language_name,
        is_jsx_like_file,
        &tree,
        source,
        &unique_nodes,
        file_id,
        &mut result_edges,
        &mut edge_keys,
        flags,
        &mut callsite_ordinals,
    );
    append_manual_precise_call_edges(
        language_config.language_name,
        &tree,
        source,
        &unique_nodes,
        file_id,
        &mut result_edges,
        &mut edge_keys,
        flags,
        &mut callsite_ordinals,
    );
    append_manual_c_enum_member_edges(
        language_config.language_name,
        &tree,
        source,
        &unique_nodes,
        file_id,
        &mut result_edges,
        &mut edge_keys,
        flags,
    );
    let manual_member_specs = language_member_specs(language_config.language_name, &tree, source);
    let local_member_targets = append_manual_member_edges(
        ManualMemberEdgeContext {
            specs: &manual_member_specs,
            unique_nodes: &unique_nodes,
            file_id,
            flags,
        },
        &mut result_edges,
        &mut edge_keys,
    );
    if language_config.language_name == "python" {
        annotate_python_context_manager_self_return_members(
            &tree,
            source,
            &unique_nodes,
            file_id,
            &mut result_edges,
            &mut edge_keys,
            flags,
        );
    }
    append_manual_receiver_call_edges(
        language_config.language_name,
        &tree,
        source,
        &unique_nodes,
        file_id,
        &mut result_edges,
        &mut edge_keys,
        flags,
        &mut callsite_ordinals,
    );
    append_manual_type_usage_edges(
        language_config.language_name,
        &tree,
        source,
        &mut unique_nodes,
        file_id,
        &file_name,
        &mut result_edges,
        &mut edge_keys,
        flags,
    );
    append_runtime_import_edges(
        &runtime_import_specs,
        &unique_nodes,
        file_id,
        &mut result_edges,
        &mut edge_keys,
        flags,
    );
    annotate_exact_runtime_import_bare_calls(
        &runtime_import_specs,
        &unique_nodes,
        &mut result_edges,
        &mut edge_keys,
        flags,
    );
    append_schema_endpoint_call_edges(
        language_config.language_name,
        source,
        file_id,
        flags,
        &mut SchemaEndpointEdgeSinks {
            unique_nodes: &mut unique_nodes,
            result_edges: &mut result_edges,
            edge_keys: &mut edge_keys,
            callsite_ordinals: &mut callsite_ordinals,
        },
    );
    append_tauri_command_registrations(
        language_config.language_name,
        source,
        file_id,
        flags,
        &mut FrameworkSymbolSinks {
            unique_nodes: &mut unique_nodes,
            result_edges: &mut result_edges,
            result_occurrences: &mut result_occurrences,
            component_access_by_node_id: &mut component_access_by_node_id,
            edge_keys: &mut edge_keys,
            callsite_ordinals: &mut callsite_ordinals,
        },
    );
    append_payload_collection_symbols(
        language_config.language_name,
        source,
        file_id,
        flags,
        &mut FrameworkSymbolSinks {
            unique_nodes: &mut unique_nodes,
            result_edges: &mut result_edges,
            result_occurrences: &mut result_occurrences,
            component_access_by_node_id: &mut component_access_by_node_id,
            edge_keys: &mut edge_keys,
            callsite_ordinals: &mut callsite_ordinals,
        },
    );
    append_framework_routes(
        path,
        language_config,
        &tree,
        source,
        file_id,
        flags,
        &mut FrameworkRouteSinks {
            unique_nodes: &mut unique_nodes,
            result_edges: &mut result_edges,
            result_occurrences: &mut result_occurrences,
            component_access_by_node_id: &mut component_access_by_node_id,
            edge_keys: &mut edge_keys,
            callsite_ordinals: &mut callsite_ordinals,
        },
    )?;

    if language_config.language_name == "rust" {
        apply_rust_receiver_call_hints(&tree, source, &mut unique_nodes);
    }

    apply_go_receiver_method_identities(
        language_config.language_name,
        &mut unique_nodes,
        &manual_member_specs,
        &local_member_targets,
        &canonical_role_by_node_id,
    );

    if !unique_nodes.is_empty() {
        result_nodes.extend(unique_nodes.values().cloned());
    }
    result_occurrences.extend(definition_occurrences(
        &unique_nodes,
        &canonical_role_by_node_id,
        file_id,
    ));

    // 3. Resolve qualified names, canonicalize IDs, and remap projections.
    let post_processed = post_process_index_results(
        result_nodes,
        &mut result_edges,
        &mut result_occurrences,
        &file_name,
        file_id,
        language_config.language_name,
        &canonical_role_by_node_id,
        is_jsx_like_file,
        &runtime_import_specs,
        flags,
    );
    let final_nodes = post_processed.nodes;
    let id_remap = post_processed.id_remap;
    let final_node_ids = final_nodes
        .iter()
        .map(|node| node.id)
        .collect::<HashSet<_>>();
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
        .filter(|(_, role)| **role == CanonicalNodeRole::ImplAnchor)
        .map(|(node_id, _)| id_remap.get(node_id).copied().unwrap_or(*node_id))
        .collect::<Vec<_>>();
    impl_anchor_node_ids.sort_unstable();
    impl_anchor_node_ids.dedup();

    let mut final_nodes = final_nodes;
    let mut component_access = component_access;
    reconcile_local_impl_anchor_nodes(
        &mut final_nodes,
        &mut result_edges,
        &mut result_occurrences,
        &mut component_access,
        &mut impl_anchor_node_ids,
    );

    let callable_projection_states =
        build_callable_projection_states(&final_nodes, &result_edges, &result_occurrences);

    if let Some(st) = &symbol_table {
        for node in &final_nodes {
            st.insert(node.id.0, node.kind);
        }
    }

    let resolution_inputs = proof_resolution::collect_call_resolution_inputs(
        &tree,
        source,
        raw_source_sha256,
        path,
        language_config.language_name,
        &resolution_parser_fingerprint(language_config),
        file_id,
        &final_nodes,
    );

    Ok((
        IndexResult {
            files: result_files,
            nodes: final_nodes,
            edges: result_edges,
            occurrences: result_occurrences,
            component_access,
            callable_projection_states,
            impl_anchor_node_ids,
        },
        resolution_inputs.calls,
        resolution_inputs.file,
    ))
}

/// Return the public language-support profile for a file extension.
pub fn language_support_profile_for_ext(ext: &str) -> Option<LanguageSupportProfile> {
    codestory_contracts::language_support::language_support_profile_for_ext(ext).copied()
}

/// Return the public language-support profile for a language name.
pub fn language_support_profile_for_language_name(
    language_name: &str,
) -> Option<LanguageSupportProfile> {
    codestory_contracts::language_support::language_support_profile_for_language_name(language_name)
        .copied()
}

/// Return parser-backed indexer configuration for an extension.
///
/// `None` can still be supported by a structural collector or diagnostic path;
/// consult the language-support profile before presenting support tiers.
pub fn get_language_for_ext(ext: &str) -> Option<LanguageConfig> {
    language_configs::get_language_for_ext(ext)
}

/// Generate a stable deterministic id from a canonical graph name.
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

fn refresh_callsite_identity(edge: &mut Edge) {
    if edge.kind != EdgeKind::CALL {
        return;
    }
    let existing = edge.callsite_identity.take();
    let col = existing.as_deref().and_then(callsite_identity_start_col);
    let marker_parts = existing
        .as_deref()
        .into_iter()
        .flat_map(|identity| identity.split('|').skip(1))
        .map(str::to_string)
        .collect::<Vec<_>>();
    edge.callsite_identity =
        canonical_callsite_identity(edge.file_node_id, edge.line, col, edge.target);
    for marker in marker_parts {
        append_callsite_part(edge, &marker);
    }
}

fn append_callsite_marker(edge: &mut Edge, marker: &'static str) {
    append_callsite_part(edge, marker);
}

fn append_callsite_part(edge: &mut Edge, marker: &str) {
    if edge.kind != EdgeKind::CALL {
        return;
    }

    match edge.callsite_identity.as_mut() {
        Some(identity) => {
            if !identity.split('|').any(|part| part == marker) {
                identity.push('|');
                identity.push_str(marker);
            }
        }
        None => edge.callsite_identity = Some(marker.to_string()),
    }
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
            let placeholder_source = edge.source == edge.target;
            if placeholder_source
                && let (Some(file_id), Some(line)) = (edge.file_node_id, edge.line)
                && let Some(ranges) = functions_by_file.get(&file_id)
                && let Some(best) = ranges
                    .iter()
                    .filter(|range| line >= range.start && line <= range.end)
                    .min_by_key(|range| (range.end - range.start, range.start))
            {
                edge.source = best.id;
            }
            if placeholder_source && call_edge_still_has_unresolved_placeholder(edge, &callable_ids)
            {
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

fn call_edge_still_has_unresolved_placeholder(edge: &Edge, callable_ids: &HashSet<NodeId>) -> bool {
    !callable_ids.contains(&edge.source) || edge.source == edge.target
}

pub(crate) mod projection;
use projection::*;
pub use projection::{CALLABLE_OUTLINE_SIGNATURE_TAG, CALLABLE_SHAPE_SIGNATURE_TAG};

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

fn graph_relation_sink_is_reference(kind: EdgeKind) -> bool {
    match kind {
        EdgeKind::MEMBER | EdgeKind::UNKNOWN => false,
        EdgeKind::TYPE_USAGE
        | EdgeKind::USAGE
        | EdgeKind::CALL
        | EdgeKind::INHERITANCE
        | EdgeKind::OVERRIDE
        | EdgeKind::TYPE_ARGUMENT
        | EdgeKind::TEMPLATE_SPECIALIZATION
        | EdgeKind::INCLUDE
        | EdgeKind::IMPORT
        | EdgeKind::MACRO_USAGE
        | EdgeKind::ANNOTATION_USAGE => true,
    }
}

fn generate_edge_id(source: i64, target: i64, kind: codestory_contracts::graph::EdgeKind) -> i64 {
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
    kind: codestory_contracts::graph::EdgeKind,
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
mod tests;

#[cfg(test)]
mod proof_resolution_cache_tests {
    use super::*;
    use crate::cache::{
        CachedCallResolutionInput, CachedClassBinding, CachedClassDeclaration, CachedClassMethod,
        CachedDeclarationKind, CachedDirectExport, CachedIndexArtifact, CachedInherentMethod,
        CachedPhpNamespace, CachedResolutionBinding, CachedResolutionFile,
        CachedTopLevelDeclaration,
    };
    use codestory_contracts::proof_resolution::{CalleeForm, ExactCallsite, FileId};
    use codestory_store::{FileInfo, FileRole};

    #[test]
    fn rebases_every_cached_receiver_inventory_and_binding_node_id() {
        let old_file = NodeId(1);
        let old_owner = NodeId(2);
        let old_method = NodeId(3);
        let old_import = NodeId(4);
        let old_caller = NodeId(5);
        let node = |id: NodeId, kind: NodeKind, name: &str| Node {
            id,
            kind,
            serialized_name: name.to_owned(),
            file_node_id: (id != old_file).then_some(old_file),
            start_line: Some(id.0 as u32),
            start_col: Some(1),
            end_line: Some(id.0 as u32),
            end_col: Some(10),
            ..Default::default()
        };
        let callsite = ExactCallsite {
            file_id: FileId(old_file.0),
            source_sha256: "a".repeat(64),
            start_byte: 20,
            end_byte_exclusive: 26,
            line: 5,
            column: 1,
            callee_form: CalleeForm::ExplicitReceiver,
            raw_target: "target".to_owned(),
        };
        let artifact = CachedIndexArtifact {
            resolution_input_schema_version: 7,
            files: vec![FileInfo {
                id: old_file.0,
                path: "old.ts".into(),
                language: "typescript".to_owned(),
                modification_time: 0,
                indexed: true,
                complete: true,
                line_count: 5,
                file_role: FileRole::Source,
            }],
            nodes: vec![
                node(old_file, NodeKind::FILE, "old.ts"),
                node(old_owner, NodeKind::CLASS, "C"),
                node(old_method, NodeKind::METHOD, "C.target"),
                node(old_import, NodeKind::UNKNOWN, "C"),
                node(old_caller, NodeKind::FUNCTION, "caller"),
            ],
            edges: Vec::new(),
            occurrences: Vec::new(),
            component_access: Vec::new(),
            callable_projection_states: Vec::new(),
            impl_anchor_node_ids: Vec::new(),
            call_resolution_inputs: vec![
                CachedCallResolutionInput {
                    callsite: callsite.clone(),
                    caller: Some(old_caller),
                    binding: CachedResolutionBinding::ConstructorBinding {
                        class_binding: CachedClassBinding::SameFile {
                            owner: old_owner,
                            owner_name: "C".to_owned(),
                        },
                        method_name: "target".to_owned(),
                    },
                    language: "typescript".to_owned(),
                    adapter_version: "reference-v9".to_owned(),
                    parser_fingerprint: "b".repeat(64),
                },
                CachedCallResolutionInput {
                    callsite,
                    caller: Some(old_caller),
                    binding: CachedResolutionBinding::ExplicitReceiverType {
                        class_binding: CachedClassBinding::StaticImport {
                            import: old_import,
                            module_specifier: "./other".to_owned(),
                            imported_name: "C".to_owned(),
                            is_default: false,
                        },
                        method_name: "target".to_owned(),
                    },
                    language: "typescript".to_owned(),
                    adapter_version: "reference-v9".to_owned(),
                    parser_fingerprint: "b".repeat(64),
                },
            ],
            resolution_file: Some(CachedResolutionFile {
                file_id: old_file,
                source_sha256: "a".repeat(64),
                language: "typescript".to_owned(),
                adapter_version: "reference-v9".to_owned(),
                parser_fingerprint: "b".repeat(64),
                complete: true,
                lookup_input_complete: true,
                typescript_module: true,
                top_level_declarations: vec![CachedTopLevelDeclaration {
                    name: "target".to_owned(),
                    declaration: old_method,
                    module_path: Vec::new(),
                    cross_module_visible: false,
                }],
                inherent_methods: vec![CachedInherentMethod {
                    owner_name: "C".to_owned(),
                    method_name: "target".to_owned(),
                    declaration: old_method,
                    module_path: Vec::new(),
                    owner: Some(old_owner),
                    has_self: true,
                    return_owner: None,
                    domain_complete: true,
                    cross_module_visible: false,
                }],
                classes: vec![CachedClassDeclaration {
                    name: "C".to_owned(),
                    declaration: old_owner,
                    methods: vec![CachedClassMethod {
                        name: "target".to_owned(),
                        declaration: old_method,
                        cross_module_visible: false,
                    }],
                    cross_module_visible: false,
                    runtime_closed: false,
                    super_name: None,
                }],
                direct_exports: vec![CachedDirectExport {
                    exported_name: "C".to_owned(),
                    declaration: old_owner,
                    is_default: false,
                    declaration_kind: CachedDeclarationKind::Class,
                }],
                export_poison_all: false,
                poisoned_export_names: vec!["unrelated".to_owned()],
                rust_modules: Vec::new(),
                rust_types: Vec::new(),
                rust_uses: Vec::new(),
                go_package: None,
                java_kotlin_package: None,
                php_namespace: CachedPhpNamespace::Invalid,
                c_cpp_file: None,
            }),
        };

        let rebased = rebase_cached_index_artifact(
            artifact,
            Path::new("/tmp/rebased.ts"),
            "export class C { target() {} }",
            "typescript",
            index_feature_flags(),
        );
        let id = |kind, name: &str| {
            rebased
                .nodes
                .iter()
                .find(|node| node.kind == kind && node.serialized_name.ends_with(name))
                .expect("rebased node")
                .id
        };
        let new_file = id(NodeKind::FILE, "rebased.ts");
        let new_owner = id(NodeKind::CLASS, "C");
        let new_method = id(NodeKind::METHOD, "C.target");
        let new_import = id(NodeKind::UNKNOWN, "C");
        let new_caller = id(NodeKind::FUNCTION, "caller");
        assert_ne!(
            (new_file, new_owner, new_method, new_import, new_caller),
            (old_file, old_owner, old_method, old_import, old_caller)
        );
        assert!(rebased.call_resolution_inputs.iter().all(|input| {
            input.callsite.file_id == FileId(new_file.0) && input.caller == Some(new_caller)
        }));
        assert!(matches!(
            &rebased.call_resolution_inputs[0].binding,
            CachedResolutionBinding::ConstructorBinding {
                class_binding: CachedClassBinding::SameFile { owner, .. }, ..
            } if *owner == new_owner
        ));
        assert!(matches!(
            &rebased.call_resolution_inputs[1].binding,
            CachedResolutionBinding::ExplicitReceiverType {
                class_binding: CachedClassBinding::StaticImport { import, .. }, ..
            } if *import == new_import
        ));
        let file = rebased.resolution_file.expect("resolution file");
        assert_eq!(file.file_id, new_file);
        assert_eq!(file.top_level_declarations[0].declaration, new_method);
        assert_eq!(file.inherent_methods[0].declaration, new_method);
        assert_eq!(file.classes[0].declaration, new_owner);
        assert_eq!(file.classes[0].methods[0].declaration, new_method);
        assert_eq!(file.direct_exports[0].declaration, new_owner);
        assert!(!file.export_poison_all);
        assert_eq!(file.poisoned_export_names, ["unrelated"]);
    }
}
