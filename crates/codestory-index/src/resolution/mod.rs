use crate::semantic::{
    SemanticCandidateIndex, SemanticCandidateNodeSnapshot, SemanticResolutionCandidate,
    SemanticResolutionRequest, SemanticResolverRegistry,
    detect_language as semantic_detect_language,
};
use anyhow::Result;
use codestory_core::{EdgeKind, NodeKind, ResolutionCertainty};
use codestory_storage::Storage;
use rayon::prelude::*;
#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{params, params_from_iter, types::Value};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use std::time::Instant;

mod candidate_selection;
mod pipeline;
mod query_helpers;
mod sql;

use query_helpers::{import_alias_mismatch, sorted_scope_file_ids};

type UnresolvedEdgeRow = (
    i64,
    Option<i64>,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
);

struct SemanticEdgeLookup<'a> {
    edge_kind: EdgeKind,
    file_id: Option<i64>,
    file_path: Option<&'a str>,
    caller_qualified: Option<&'a str>,
    source_name: &'a str,
    target_name: &'a str,
    callsite_identity: Option<&'a str>,
}

const SCOPED_CALLER_TABLE: &str = "resolution_scoped_caller_ids";

type SameFileCacheKey = (i64, String, String);
type SameModuleCacheKey = (String, String, String);
type NameCacheKey = (String, String);
const RESOLUTION_SUPPORT_SNAPSHOT_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SemanticResolutionRequestKey {
    edge_kind: i32,
    file_id: Option<i64>,
    file_path: Option<String>,
    caller_qualified: Option<String>,
    target_name: String,
}

#[derive(Default, Debug, Clone, Copy)]
struct SemanticRequestStats {
    total_requests: usize,
    unique_requests: usize,
    skipped_requests: usize,
}

#[cfg(test)]
#[allow(dead_code)]
#[derive(Default)]
struct ResolutionLookupCache {
    same_file_lookup: HashMap<(String, i64, String), Option<i64>>,
    same_module_lookup: HashMap<(String, String, String), Option<i64>>,
    global_unique_lookup: HashMap<(String, String), Option<i64>>,
}

#[derive(Debug, Clone)]
struct CandidateNode {
    id: i64,
    file_node_id: Option<i64>,
    serialized_name: String,
    serialized_name_ascii_lower: String,
    qualified_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CandidateNodeSnapshot {
    id: i64,
    file_node_id: Option<i64>,
    serialized_name: String,
    qualified_name: Option<String>,
}

#[derive(Default, Debug)]
struct CandidateIndex {
    nodes: Vec<CandidateNode>,
    node_offset_by_id: HashMap<i64, usize>,
    exact_map: HashMap<String, Vec<usize>>,
    suffix_map_ascii_lower: HashMap<String, Vec<usize>>,
    same_file_cache: RwLock<HashMap<SameFileCacheKey, Option<i64>>>,
    same_module_cache: RwLock<HashMap<SameModuleCacheKey, Option<i64>>>,
    global_unique_cache: RwLock<HashMap<NameCacheKey, Option<i64>>>,
    fuzzy_cache: RwLock<HashMap<NameCacheKey, Option<i64>>>,
}

#[derive(Debug, Clone)]
struct ResolvedEdgeUpdate {
    edge_id: i64,
    resolved_target_node_id: Option<i64>,
    confidence: Option<f32>,
    certainty: Option<&'static str>,
    candidate_payload: Option<String>,
}

#[derive(Default, Debug)]
pub struct ResolutionStats {
    pub unresolved_calls_before: usize,
    pub resolved_calls: usize,
    pub unresolved_calls: usize,
    pub unresolved_imports_before: usize,
    pub resolved_imports: usize,
    pub unresolved_imports: usize,
    pub telemetry: ResolutionPhaseTelemetry,
    pub strategy_counters: ResolutionStrategyCounters,
}

struct PreparedResolutionState {
    call_candidate_index: CandidateIndex,
    import_candidate_index: CandidateIndex,
    call_semantic_index: SemanticCandidateIndex,
    import_semantic_index: SemanticCandidateIndex,
    override_support: OverrideSupport,
}

type OwnerByMethod = HashMap<i64, Vec<i64>>;
type MethodsByOwnerAndName = HashMap<(i64, String), Vec<i64>>;
type OwnerNameById = HashMap<i64, String>;
type MethodsByOwnerNameAndName = HashMap<(String, String), Vec<i64>>;

#[derive(Debug, Clone, Default)]
pub(super) struct OverrideSupport {
    pub(super) owner_by_method: OwnerByMethod,
    pub(super) methods_by_owner_and_name: MethodsByOwnerAndName,
    pub(super) owner_name_by_id: OwnerNameById,
    pub(super) methods_by_owner_name_and_name: MethodsByOwnerNameAndName,
    pub(super) inheritance_by_type: HashMap<i64, Vec<i64>>,
    pub(super) inheritance_by_owner_name: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OverrideMemberSnapshot {
    owner_id: i64,
    owner_name: String,
    method_id: i64,
    method_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResolutionSupportSnapshot {
    #[serde(default)]
    enable_semantic: bool,
    call_candidates: Vec<CandidateNodeSnapshot>,
    import_candidates: Vec<CandidateNodeSnapshot>,
    call_semantic_nodes: Vec<SemanticCandidateNodeSnapshot>,
    import_semantic_nodes: Vec<SemanticCandidateNodeSnapshot>,
    override_members: Vec<OverrideMemberSnapshot>,
    override_inheritance: Vec<(i64, i64)>,
    override_inheritance_by_name: Vec<(String, String)>,
    node_names: Vec<(i64, String)>,
}

impl OverrideSupport {
    fn from_snapshot(
        override_members: Vec<OverrideMemberSnapshot>,
        override_inheritance: Vec<(i64, i64)>,
        override_inheritance_by_name: Vec<(String, String)>,
        node_names: Vec<(i64, String)>,
    ) -> Self {
        let mut owner_by_method = OwnerByMethod::new();
        let mut methods_by_owner_and_name = MethodsByOwnerAndName::new();
        let mut owner_name_by_id = OwnerNameById::new();
        let mut methods_by_owner_name_and_name = MethodsByOwnerNameAndName::new();
        for entry in override_members {
            owner_by_method
                .entry(entry.method_id)
                .or_default()
                .push(entry.owner_id);
            owner_name_by_id
                .entry(entry.owner_id)
                .or_insert(entry.owner_name.clone());
            methods_by_owner_and_name
                .entry((entry.owner_id, entry.method_name.clone()))
                .or_default()
                .push(entry.method_id);
            methods_by_owner_name_and_name
                .entry((entry.owner_name, entry.method_name))
                .or_default()
                .push(entry.method_id);
        }
        for (node_id, node_name) in node_names {
            owner_name_by_id.entry(node_id).or_insert(node_name);
        }

        let mut inheritance_by_type = HashMap::<i64, Vec<i64>>::new();
        for (source_id, target_id) in override_inheritance {
            inheritance_by_type
                .entry(source_id)
                .or_default()
                .push(target_id);
        }

        let mut inheritance_by_owner_name = HashMap::<String, Vec<String>>::new();
        for (source_name, target_name) in override_inheritance_by_name {
            inheritance_by_owner_name
                .entry(source_name)
                .or_default()
                .push(target_name);
        }

        Self {
            owner_by_method,
            methods_by_owner_and_name,
            owner_name_by_id,
            methods_by_owner_name_and_name,
            inheritance_by_type,
            inheritance_by_owner_name,
        }
    }

    fn override_member_rows(&self) -> Vec<OverrideMemberSnapshot> {
        let mut rows = Vec::new();
        for ((owner_id, method_name), method_ids) in &self.methods_by_owner_and_name {
            let owner_name = self
                .owner_name_by_id
                .get(owner_id)
                .cloned()
                .unwrap_or_default();
            for method_id in method_ids {
                rows.push(OverrideMemberSnapshot {
                    owner_id: *owner_id,
                    owner_name: owner_name.clone(),
                    method_id: *method_id,
                    method_name: method_name.clone(),
                });
            }
        }
        rows
    }

    fn override_inheritance_rows(&self) -> Vec<(i64, i64)> {
        let mut rows = Vec::new();
        for (source_id, target_ids) in &self.inheritance_by_type {
            for target_id in target_ids {
                rows.push((*source_id, *target_id));
            }
        }
        rows
    }

    fn override_inheritance_by_name_rows(&self) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        for (source_name, target_names) in &self.inheritance_by_owner_name {
            for target_name in target_names {
                rows.push((source_name.clone(), target_name.clone()));
            }
        }
        rows
    }

    fn node_name_rows(&self) -> Vec<(i64, String)> {
        self.owner_name_by_id
            .iter()
            .map(|(node_id, node_name)| (*node_id, node_name.clone()))
            .collect()
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct ResolutionPhaseTelemetry {
    pub scope_prepare_ms: u64,
    pub unresolved_count_start_ms: u64,
    pub unresolved_override_count_ms: u64,
    pub support_snapshot_load_ms: u64,
    pub support_snapshot_store_ms: u64,
    pub support_snapshot_hit: bool,
    pub call_prepare_ms: u64,
    pub call_cleanup_ms: u64,
    pub call_unresolved_load_ms: u64,
    pub call_candidate_index_ms: u64,
    pub call_semantic_index_ms: u64,
    pub call_semantic_candidates_ms: u64,
    pub call_compute_ms: u64,
    pub call_apply_ms: u64,
    pub import_prepare_ms: u64,
    pub import_unresolved_load_ms: u64,
    pub import_candidate_index_ms: u64,
    pub import_semantic_index_ms: u64,
    pub import_semantic_candidates_ms: u64,
    pub import_semantic_requests: usize,
    pub import_semantic_unique_requests: usize,
    pub import_semantic_skipped_requests: usize,
    pub import_compute_ms: u64,
    pub import_apply_ms: u64,
    pub call_semantic_requests: usize,
    pub call_semantic_unique_requests: usize,
    pub call_semantic_skipped_requests: usize,
    pub override_resolution_ms: u64,
    pub unresolved_count_end_ms: u64,
}

impl ResolutionPhaseTelemetry {
    fn record_semantic_request_stats(&mut self, edge_kind: EdgeKind, stats: SemanticRequestStats) {
        match edge_kind {
            EdgeKind::CALL => {
                self.call_semantic_requests = self
                    .call_semantic_requests
                    .saturating_add(stats.total_requests);
                self.call_semantic_unique_requests = self
                    .call_semantic_unique_requests
                    .saturating_add(stats.unique_requests);
                self.call_semantic_skipped_requests = self
                    .call_semantic_skipped_requests
                    .saturating_add(stats.skipped_requests);
            }
            EdgeKind::IMPORT => {
                self.import_semantic_requests = self
                    .import_semantic_requests
                    .saturating_add(stats.total_requests);
                self.import_semantic_unique_requests = self
                    .import_semantic_unique_requests
                    .saturating_add(stats.unique_requests);
                self.import_semantic_skipped_requests = self
                    .import_semantic_skipped_requests
                    .saturating_add(stats.skipped_requests);
            }
            _ => {}
        }
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct ResolutionStrategyCounters {
    pub call_same_file: usize,
    pub call_same_module: usize,
    pub call_global_unique: usize,
    pub call_semantic_fallback: usize,
    pub import_same_file: usize,
    pub import_same_module: usize,
    pub import_global_unique: usize,
    pub import_fuzzy: usize,
    pub import_semantic_fallback: usize,
}

impl ResolutionStrategyCounters {
    fn record(&mut self, strategy: Option<ResolutionStrategy>) {
        match strategy {
            Some(ResolutionStrategy::CallSameFile) => self.call_same_file += 1,
            Some(ResolutionStrategy::CallSameModule) => self.call_same_module += 1,
            Some(ResolutionStrategy::CallGlobalUnique) => self.call_global_unique += 1,
            Some(ResolutionStrategy::CallSemanticFallback) => self.call_semantic_fallback += 1,
            Some(ResolutionStrategy::ImportSameFile) => self.import_same_file += 1,
            Some(ResolutionStrategy::ImportSameModule) => self.import_same_module += 1,
            Some(ResolutionStrategy::ImportGlobalUnique) => self.import_global_unique += 1,
            Some(ResolutionStrategy::ImportFuzzy) => self.import_fuzzy += 1,
            Some(ResolutionStrategy::ImportSemanticFallback) => self.import_semantic_fallback += 1,
            None => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolutionStrategy {
    CallSameFile,
    CallSameModule,
    CallGlobalUnique,
    CallSemanticFallback,
    ImportSameFile,
    ImportSameModule,
    ImportGlobalUnique,
    ImportFuzzy,
    ImportSemanticFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeCallerMode {
    Unscoped,
    ScopedCallers,
    Empty,
}

#[derive(Debug, Clone, Copy)]
struct ScopeCallerContext {
    mode: ScopeCallerMode,
}

impl ScopeCallerContext {
    fn unscoped() -> Self {
        Self {
            mode: ScopeCallerMode::Unscoped,
        }
    }

    fn prepare(
        conn: &rusqlite::Connection,
        caller_scope_file_ids: Option<&HashSet<i64>>,
    ) -> Result<Self> {
        let Some(scope_ids) = sorted_scope_file_ids(caller_scope_file_ids) else {
            return Ok(Self::unscoped());
        };
        if scope_ids.is_empty() {
            return Ok(Self {
                mode: ScopeCallerMode::Empty,
            });
        }

        conn.execute_batch(&format!(
            "CREATE TEMP TABLE IF NOT EXISTS {SCOPED_CALLER_TABLE} (
                caller_id INTEGER PRIMARY KEY
             );
             DELETE FROM {SCOPED_CALLER_TABLE};"
        ))?;
        let mut query = format!(
            "INSERT INTO {SCOPED_CALLER_TABLE} (caller_id)
             SELECT id FROM node WHERE file_node_id IN ("
        );
        query.push_str(&sql::numbered_placeholders(1, scope_ids.len()));
        query.push(')');
        conn.execute(&query, params_from_iter(scope_ids.iter()))?;

        Ok(Self {
            mode: ScopeCallerMode::ScopedCallers,
        })
    }

    fn is_empty(&self) -> bool {
        matches!(self.mode, ScopeCallerMode::Empty)
    }

    fn is_scoped(&self) -> bool {
        matches!(self.mode, ScopeCallerMode::ScopedCallers)
    }
}

#[derive(Debug, Clone)]
struct PreparedName {
    original: String,
    ascii_lower: String,
}

impl PreparedName {
    fn new(value: String) -> Self {
        let ascii_lower = value.to_ascii_lowercase();
        Self {
            original: value,
            ascii_lower,
        }
    }
}

#[derive(Default, Debug, Clone)]
struct OrderedCandidateIds {
    ordered: Vec<i64>,
    seen: HashSet<i64>,
}

impl OrderedCandidateIds {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            ordered: Vec::with_capacity(capacity),
            seen: HashSet::with_capacity(capacity.saturating_mul(2)),
        }
    }

    fn push(&mut self, candidate: i64) {
        if self.seen.insert(candidate) {
            self.ordered.push(candidate);
        }
    }

    fn len(&self) -> usize {
        self.ordered.len()
    }

    fn as_slice(&self) -> &[i64] {
        &self.ordered
    }

    fn into_vec(self) -> Vec<i64> {
        self.ordered
    }

    fn extend_stage(&mut self, stage_candidates: &[i64], limit: usize) {
        if self.len() >= limit {
            return;
        }
        for candidate in stage_candidates {
            self.push(*candidate);
            if self.len() >= limit {
                return;
            }
        }
    }
}

#[derive(Debug)]
struct ComputedResolution {
    update: ResolvedEdgeUpdate,
    strategy: Option<ResolutionStrategy>,
}

#[derive(Debug, Clone, Copy)]
struct ResolutionFlags {
    legacy_mode: bool,
    enable_semantic: bool,
    store_candidates: bool,
    parallel_compute: bool,
}

impl ResolutionFlags {
    fn from_env() -> Self {
        let legacy_mode = env_flag("CODESTORY_RESOLUTION_LEGACY_MODE", false)
            || env_flag("CODESTORY_RESOLUTION_LEGACY", false);
        Self {
            legacy_mode,
            enable_semantic: env_flag("CODESTORY_RESOLUTION_ENABLE_SEMANTIC", !legacy_mode),
            store_candidates: env_flag("CODESTORY_RESOLUTION_STORE_CANDIDATES", false),
            parallel_compute: env_flag("CODESTORY_RESOLUTION_PARALLEL_COMPUTE", !legacy_mode),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ResolutionPolicy {
    call_same_file: f32,
    call_same_module: f32,
    call_global_unique: f32,
    import_same_file: f32,
    import_same_module: f32,
    import_global_unique: f32,
    import_fuzzy: f32,
    min_call_confidence: f32,
}

impl ResolutionPolicy {
    fn for_flags(flags: ResolutionFlags) -> Self {
        if flags.legacy_mode {
            Self {
                call_same_file: 0.95,
                call_same_module: 0.80,
                call_global_unique: 0.60,
                import_same_file: 0.90,
                import_same_module: 0.70,
                import_global_unique: 0.50,
                import_fuzzy: 0.30,
                min_call_confidence: 0.40,
            }
        } else {
            Self {
                call_same_file: 0.95,
                call_same_module: 0.80,
                call_global_unique: 0.62,
                import_same_file: 0.90,
                import_same_module: 0.75,
                import_global_unique: 0.55,
                import_fuzzy: 0.35,
                min_call_confidence: ResolutionCertainty::PROBABLE_MIN,
            }
        }
    }
}

pub struct ResolutionPass {
    flags: ResolutionFlags,
    policy: ResolutionPolicy,
    semantic_resolvers: SemanticResolverRegistry,
}

impl Default for ResolutionPass {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolutionPass {
    pub fn new() -> Self {
        let flags = ResolutionFlags::from_env();
        let policy = ResolutionPolicy::for_flags(flags);
        Self {
            flags,
            policy,
            semantic_resolvers: SemanticResolverRegistry::new(flags.enable_semantic),
        }
    }

    pub fn run(&self, storage: &mut Storage) -> Result<ResolutionStats> {
        self.run_with_scope(storage, None)
    }

    pub fn run_with_scope(
        &self,
        storage: &mut Storage,
        caller_scope_file_ids: Option<&HashSet<i64>>,
    ) -> Result<ResolutionStats> {
        let conn = storage.get_connection();
        run_in_immediate_transaction(conn, |conn| {
            let mut telemetry = ResolutionPhaseTelemetry::default();
            let scope_started = Instant::now();
            let scope_context = ScopeCallerContext::prepare(conn, caller_scope_file_ids)?;
            telemetry.scope_prepare_ms = duration_ms_u64(scope_started.elapsed());

            let counts_started = Instant::now();
            let unresolved_calls_before =
                Self::count_unresolved_on_conn(conn, EdgeKind::CALL, &scope_context)?;
            let unresolved_imports_before =
                Self::count_unresolved_on_conn(conn, EdgeKind::IMPORT, &scope_context)?;
            telemetry.unresolved_count_start_ms = duration_ms_u64(counts_started.elapsed());
            let override_count_started = Instant::now();
            let _unresolved_overrides_before =
                Self::count_unresolved_on_conn(conn, EdgeKind::OVERRIDE, &scope_context)?;
            telemetry.unresolved_override_count_ms =
                duration_ms_u64(override_count_started.elapsed());

            let prepared = PreparedResolutionState::load(storage, self.flags, &mut telemetry)?;

            let mut strategy_counters = ResolutionStrategyCounters::default();
            let resolved_calls = self.resolve_calls_on_conn(
                conn,
                &scope_context,
                &prepared,
                &mut telemetry,
                &mut strategy_counters,
            )?;
            let resolved_imports = self.resolve_imports_on_conn(
                conn,
                &scope_context,
                &prepared,
                &mut telemetry,
                &mut strategy_counters,
            )?;
            let _resolved_overrides =
                self.resolve_overrides_on_conn(conn, &scope_context, &prepared, &mut telemetry)?;

            let counts_finished = Instant::now();
            let unresolved_calls =
                Self::count_unresolved_on_conn(conn, EdgeKind::CALL, &scope_context)?;
            let unresolved_imports =
                Self::count_unresolved_on_conn(conn, EdgeKind::IMPORT, &scope_context)?;
            telemetry.unresolved_count_end_ms = duration_ms_u64(counts_finished.elapsed());

            Ok(ResolutionStats {
                unresolved_calls_before,
                resolved_calls,
                unresolved_calls,
                unresolved_imports_before,
                resolved_imports,
                unresolved_imports,
                telemetry,
                strategy_counters,
            })
        })
    }

    pub fn unresolved_counts(&self, storage: &Storage) -> Result<(usize, usize)> {
        self.unresolved_counts_with_scope(storage, None)
    }

    pub fn unresolved_counts_with_scope(
        &self,
        storage: &Storage,
        caller_scope_file_ids: Option<&HashSet<i64>>,
    ) -> Result<(usize, usize)> {
        let conn = storage.get_connection();
        let scope_context = ScopeCallerContext::prepare(conn, caller_scope_file_ids)?;
        Ok((
            Self::count_unresolved_on_conn(conn, EdgeKind::CALL, &scope_context)?,
            Self::count_unresolved_on_conn(conn, EdgeKind::IMPORT, &scope_context)?,
        ))
    }

    pub fn unresolved_edge_count_with_scope(
        &self,
        storage: &Storage,
        kind: EdgeKind,
        caller_scope_file_ids: Option<&HashSet<i64>>,
    ) -> Result<usize> {
        let conn = storage.get_connection();
        let scope_context = ScopeCallerContext::prepare(conn, caller_scope_file_ids)?;
        Self::count_unresolved_on_conn(conn, kind, &scope_context)
    }

    fn count_unresolved_on_conn(
        conn: &rusqlite::Connection,
        kind: EdgeKind,
        scope_context: &ScopeCallerContext,
    ) -> Result<usize> {
        if scope_context.is_empty() {
            return Ok(0);
        }

        let mut query = String::from("SELECT COUNT(*) FROM edge e");
        if scope_context.is_scoped() {
            query.push_str(&format!(
                " JOIN {SCOPED_CALLER_TABLE} scoped ON scoped.caller_id = e.source_node_id"
            ));
        }
        query.push_str(" WHERE e.kind = ?1 AND e.resolved_target_node_id IS NULL");

        let count: i64 = conn.query_row(&query, params![kind as i32], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn resolve_calls(&self, storage: &mut Storage) -> Result<usize> {
        self.resolve_calls_with_scope(storage, None)
    }

    pub fn resolve_calls_with_scope(
        &self,
        storage: &mut Storage,
        caller_scope_file_ids: Option<&HashSet<i64>>,
    ) -> Result<usize> {
        let conn = storage.get_connection();
        let scope_context = ScopeCallerContext::prepare(conn, caller_scope_file_ids)?;
        let mut telemetry = ResolutionPhaseTelemetry::default();
        let prepared = PreparedResolutionState::load(storage, self.flags, &mut telemetry)?;
        let mut strategy_counters = ResolutionStrategyCounters::default();
        self.resolve_calls_on_conn(
            conn,
            &scope_context,
            &prepared,
            &mut telemetry,
            &mut strategy_counters,
        )
    }

    fn resolve_calls_on_conn(
        &self,
        conn: &rusqlite::Connection,
        scope_context: &ScopeCallerContext,
        prepared: &PreparedResolutionState,
        telemetry: &mut ResolutionPhaseTelemetry,
        strategy_counters: &mut ResolutionStrategyCounters,
    ) -> Result<usize> {
        pipeline::resolve_calls_on_conn(
            self,
            conn,
            scope_context,
            prepared,
            telemetry,
            strategy_counters,
        )
    }

    pub fn resolve_imports(&self, storage: &mut Storage) -> Result<usize> {
        self.resolve_imports_with_scope(storage, None)
    }

    pub fn resolve_imports_with_scope(
        &self,
        storage: &mut Storage,
        caller_scope_file_ids: Option<&HashSet<i64>>,
    ) -> Result<usize> {
        let conn = storage.get_connection();
        let scope_context = ScopeCallerContext::prepare(conn, caller_scope_file_ids)?;
        let mut telemetry = ResolutionPhaseTelemetry::default();
        let prepared = PreparedResolutionState::load(storage, self.flags, &mut telemetry)?;
        let mut strategy_counters = ResolutionStrategyCounters::default();
        self.resolve_imports_on_conn(
            conn,
            &scope_context,
            &prepared,
            &mut telemetry,
            &mut strategy_counters,
        )
    }

    fn resolve_imports_on_conn(
        &self,
        conn: &rusqlite::Connection,
        scope_context: &ScopeCallerContext,
        prepared: &PreparedResolutionState,
        telemetry: &mut ResolutionPhaseTelemetry,
        strategy_counters: &mut ResolutionStrategyCounters,
    ) -> Result<usize> {
        pipeline::resolve_imports_on_conn(
            self,
            conn,
            scope_context,
            prepared,
            telemetry,
            strategy_counters,
        )
    }

    fn resolve_overrides_on_conn(
        &self,
        conn: &rusqlite::Connection,
        scope_context: &ScopeCallerContext,
        prepared: &PreparedResolutionState,
        telemetry: &mut ResolutionPhaseTelemetry,
    ) -> Result<usize> {
        pipeline::resolve_overrides_on_conn(self, conn, scope_context, prepared, telemetry)
    }

    fn compute_call_resolution(
        &self,
        candidate_index: &CandidateIndex,
        row: &UnresolvedEdgeRow,
        semantic_candidates: &[SemanticResolutionCandidate],
    ) -> Result<ComputedResolution> {
        candidate_selection::compute_call_resolution(
            self,
            candidate_index,
            row,
            semantic_candidates,
        )
    }

    fn compute_import_resolution(
        &self,
        candidate_index: &CandidateIndex,
        row: &UnresolvedEdgeRow,
        semantic_candidates: &[SemanticResolutionCandidate],
    ) -> Result<ComputedResolution> {
        candidate_selection::compute_import_resolution(
            self,
            candidate_index,
            row,
            semantic_candidates,
        )
    }
    fn semantic_candidates_for_edge(
        &self,
        index: &SemanticCandidateIndex,
        request_key: &SemanticResolutionRequestKey,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let request = SemanticResolutionRequest {
            edge_kind: EdgeKind::try_from(request_key.edge_kind)?,
            file_id: request_key.file_id,
            file_path: request_key.file_path.clone(),
            caller_qualified: request_key.caller_qualified.clone(),
            target_name: request_key.target_name.clone(),
        };
        self.semantic_resolvers.resolve(index, &request)
    }

    fn semantic_candidates_for_rows(
        &self,
        index: &SemanticCandidateIndex,
        rows: &[UnresolvedEdgeRow],
        edge_kind: EdgeKind,
    ) -> Result<(Vec<Vec<SemanticResolutionCandidate>>, SemanticRequestStats)> {
        if !self.flags.enable_semantic {
            return Ok((
                vec![Vec::new(); rows.len()],
                SemanticRequestStats::default(),
            ));
        }
        let mut request_indexes = Vec::with_capacity(rows.len());
        let mut unique_keys = Vec::new();
        let mut unique_key_indexes = HashMap::new();
        let mut stats = SemanticRequestStats {
            total_requests: rows.len(),
            ..Default::default()
        };

        for row in rows {
            let lookup = semantic_lookup_from_row(edge_kind, row);
            let Some(request_key) = semantic_request_key(&lookup) else {
                stats.skipped_requests += 1;
                request_indexes.push(None);
                continue;
            };

            let request_index = if let Some(existing) = unique_key_indexes.get(&request_key) {
                *existing
            } else {
                let next_index = unique_keys.len();
                unique_key_indexes.insert(request_key.clone(), next_index);
                unique_keys.push(request_key);
                next_index
            };
            request_indexes.push(Some(request_index));
        }

        stats.unique_requests = unique_keys.len();

        let unique_results: Vec<Vec<SemanticResolutionCandidate>> =
            if self.flags.parallel_compute && unique_keys.len() > 1 {
                unique_keys
                    .par_iter()
                    .map(|request_key| self.semantic_candidates_for_edge(index, request_key))
                    .collect::<Result<Vec<_>>>()?
            } else {
                unique_keys
                    .iter()
                    .map(|request_key| self.semantic_candidates_for_edge(index, request_key))
                    .collect::<Result<Vec<_>>>()?
            };

        Ok((
            request_indexes
                .into_iter()
                .map(|request_index| {
                    request_index
                        .map(|idx| unique_results[idx].clone())
                        .unwrap_or_default()
                })
                .collect(),
            stats,
        ))
    }
}

fn run_in_immediate_transaction<T, F>(conn: &rusqlite::Connection, work: F) -> Result<T>
where
    F: FnOnce(&rusqlite::Connection) -> Result<T>,
{
    conn.execute_batch("BEGIN IMMEDIATE TRANSACTION")?;
    match work(conn) {
        Ok(value) => {
            conn.execute_batch("COMMIT")?;
            Ok(value)
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

fn build_resolved_edge_update(
    edge_id: i64,
    selected: Option<(i64, f32)>,
    candidates: &[i64],
) -> Result<ResolvedEdgeUpdate> {
    let candidate_payload = candidate_json(candidates)?;
    let (resolved_target_node_id, confidence, certainty) = if let Some((target_id, confidence)) =
        selected
    {
        (
            Some(target_id),
            Some(confidence),
            ResolutionCertainty::from_confidence(Some(confidence)).map(ResolutionCertainty::as_str),
        )
    } else {
        (None, None, None)
    };
    Ok(ResolvedEdgeUpdate {
        edge_id,
        resolved_target_node_id,
        confidence,
        certainty,
        candidate_payload,
    })
}

fn collect_candidate_pool_from_index(
    index: &CandidateIndex,
    names: &[PreparedName],
    out: &mut OrderedCandidateIds,
    limit: usize,
) {
    if out.len() >= limit {
        return;
    }
    for name in names {
        for id in index.top_matches_readonly(&name.original, &name.ascii_lower, 3) {
            out.push(id);
            if out.len() >= limit {
                return;
            }
        }
    }
}

fn semantic_language_bucket(file_path: Option<&str>) -> Option<&'static str> {
    semantic_detect_language(file_path)
}

fn semantic_lookup_from_row<'a>(
    edge_kind: EdgeKind,
    row: &'a UnresolvedEdgeRow,
) -> SemanticEdgeLookup<'a> {
    let (
        _,
        file_id,
        caller_qualified,
        source_name,
        target_name,
        caller_file_path,
        callsite_identity,
    ) = row;
    SemanticEdgeLookup {
        edge_kind,
        file_id: *file_id,
        file_path: caller_file_path.as_deref(),
        caller_qualified: caller_qualified.as_deref(),
        source_name,
        target_name,
        callsite_identity: callsite_identity.as_deref(),
    }
}

fn semantic_request_key(lookup: &SemanticEdgeLookup<'_>) -> Option<SemanticResolutionRequestKey> {
    if semantic_language_bucket(lookup.file_path).is_none()
        || should_skip_semantic_candidates(lookup)
    {
        return None;
    }

    Some(SemanticResolutionRequestKey {
        edge_kind: lookup.edge_kind as i32,
        file_id: lookup.file_id,
        file_path: lookup.file_path.map(str::to_string),
        caller_qualified: lookup.caller_qualified.map(str::to_string),
        target_name: semantic_request_target_name(lookup),
    })
}

fn semantic_request_target_name(lookup: &SemanticEdgeLookup<'_>) -> String {
    if lookup.edge_kind == EdgeKind::IMPORT
        && import_alias_mismatch(lookup.source_name, lookup.target_name)
    {
        format!("{} as {}", lookup.target_name, lookup.source_name)
    } else {
        lookup.target_name.to_string()
    }
}

fn should_skip_semantic_candidates(lookup: &SemanticEdgeLookup<'_>) -> bool {
    lookup.edge_kind == EdgeKind::CALL
        && !should_keep_common_call_resolution(
            lookup.target_name,
            ResolutionCertainty::CERTAIN_MIN,
            lookup.callsite_identity,
        )
}

pub(super) fn semantic_candidate_kinds(edge_kind: EdgeKind) -> &'static [i32] {
    match edge_kind {
        EdgeKind::CALL => &[NodeKind::FUNCTION as i32, NodeKind::METHOD as i32],
        EdgeKind::IMPORT => &[
            NodeKind::MODULE as i32,
            NodeKind::NAMESPACE as i32,
            NodeKind::PACKAGE as i32,
            NodeKind::CLASS as i32,
            NodeKind::STRUCT as i32,
            NodeKind::INTERFACE as i32,
            NodeKind::ANNOTATION as i32,
            NodeKind::UNION as i32,
            NodeKind::ENUM as i32,
            NodeKind::TYPEDEF as i32,
            NodeKind::FUNCTION as i32,
            NodeKind::METHOD as i32,
        ],
        _ => &[],
    }
}

impl PreparedResolutionState {
    fn load(
        storage: &Storage,
        flags: ResolutionFlags,
        telemetry: &mut ResolutionPhaseTelemetry,
    ) -> Result<Self> {
        let snapshot_load_started = Instant::now();
        if let Some(snapshot_blob) =
            storage.get_resolution_support_snapshot(RESOLUTION_SUPPORT_SNAPSHOT_VERSION)?
        {
            match serde_json::from_slice::<ResolutionSupportSnapshot>(&snapshot_blob) {
                Ok(snapshot) if snapshot.enable_semantic == flags.enable_semantic => {
                    telemetry.support_snapshot_load_ms =
                        duration_ms_u64(snapshot_load_started.elapsed());
                    telemetry.support_snapshot_hit = true;
                    return Ok(Self::from_snapshot(snapshot, flags));
                }
                Ok(_) | Err(_) => {
                    storage.invalidate_resolution_support_snapshot()?;
                }
            }
        }
        telemetry.support_snapshot_load_ms = duration_ms_u64(snapshot_load_started.elapsed());

        let conn = storage.get_connection();
        let call_candidate_started = Instant::now();
        let call_candidate_index = CandidateIndex::load(
            conn,
            &[
                NodeKind::FUNCTION as i32,
                NodeKind::METHOD as i32,
                NodeKind::MACRO as i32,
            ],
        )?;
        telemetry.call_candidate_index_ms = duration_ms_u64(call_candidate_started.elapsed());

        let import_candidate_started = Instant::now();
        let import_candidate_index = CandidateIndex::load(
            conn,
            &[
                NodeKind::MODULE as i32,
                NodeKind::NAMESPACE as i32,
                NodeKind::PACKAGE as i32,
            ],
        )?;
        telemetry.import_candidate_index_ms = duration_ms_u64(import_candidate_started.elapsed());

        let (call_semantic_index, import_semantic_index) = if flags.enable_semantic {
            let call_semantic_started = Instant::now();
            let call_semantic_index =
                SemanticCandidateIndex::load(conn, semantic_candidate_kinds(EdgeKind::CALL))?;
            telemetry.call_semantic_index_ms = duration_ms_u64(call_semantic_started.elapsed());

            let import_semantic_started = Instant::now();
            let import_semantic_index =
                SemanticCandidateIndex::load(conn, semantic_candidate_kinds(EdgeKind::IMPORT))?;
            telemetry.import_semantic_index_ms = duration_ms_u64(import_semantic_started.elapsed());
            (call_semantic_index, import_semantic_index)
        } else {
            (
                SemanticCandidateIndex::default(),
                SemanticCandidateIndex::default(),
            )
        };

        let override_support = pipeline::load_override_support(conn)?;

        let prepared = Self {
            call_candidate_index,
            import_candidate_index,
            call_semantic_index,
            import_semantic_index,
            override_support,
        };

        let snapshot_store_started = Instant::now();
        let snapshot_blob = serde_json::to_vec(&prepared.snapshot(flags))?;
        storage
            .put_resolution_support_snapshot(RESOLUTION_SUPPORT_SNAPSHOT_VERSION, &snapshot_blob)?;
        telemetry.support_snapshot_store_ms = duration_ms_u64(snapshot_store_started.elapsed());

        Ok(prepared)
    }

    fn from_snapshot(snapshot: ResolutionSupportSnapshot, flags: ResolutionFlags) -> Self {
        let override_support = OverrideSupport::from_snapshot(
            snapshot.override_members,
            snapshot.override_inheritance,
            snapshot.override_inheritance_by_name,
            snapshot.node_names,
        );
        Self {
            call_candidate_index: CandidateIndex::from_snapshot_nodes(snapshot.call_candidates),
            import_candidate_index: CandidateIndex::from_snapshot_nodes(snapshot.import_candidates),
            call_semantic_index: if flags.enable_semantic {
                SemanticCandidateIndex::from_snapshot_nodes(snapshot.call_semantic_nodes)
            } else {
                SemanticCandidateIndex::default()
            },
            import_semantic_index: if flags.enable_semantic {
                SemanticCandidateIndex::from_snapshot_nodes(snapshot.import_semantic_nodes)
            } else {
                SemanticCandidateIndex::default()
            },
            override_support,
        }
    }

    fn snapshot(&self, flags: ResolutionFlags) -> ResolutionSupportSnapshot {
        ResolutionSupportSnapshot {
            enable_semantic: flags.enable_semantic,
            call_candidates: self.call_candidate_index.snapshot_nodes(),
            import_candidates: self.import_candidate_index.snapshot_nodes(),
            call_semantic_nodes: self.call_semantic_index.snapshot_nodes(),
            import_semantic_nodes: self.import_semantic_index.snapshot_nodes(),
            override_members: self.override_support.override_member_rows(),
            override_inheritance: self.override_support.override_inheritance_rows(),
            override_inheritance_by_name: self.override_support.override_inheritance_by_name_rows(),
            node_names: self.override_support.node_name_rows(),
        }
    }
}

impl CandidateIndex {
    fn load(conn: &rusqlite::Connection, kinds: &[i32]) -> Result<Self> {
        let kind_clause = kind_clause(kinds);
        let query = format!(
            "SELECT id, file_node_id, serialized_name, qualified_name
             FROM node
             WHERE kind IN ({})
             ORDER BY COALESCE(start_line, -9223372036854775808), id",
            kind_clause
        );
        let mut stmt = conn.prepare(&query)?;
        let rows = stmt.query_map([], |row| {
            let serialized_name: String = row.get(2)?;
            Ok(CandidateNode {
                id: row.get(0)?,
                file_node_id: row.get(1)?,
                serialized_name_ascii_lower: serialized_name.to_ascii_lowercase(),
                serialized_name,
                qualified_name: row.get(3)?,
            })
        })?;

        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }

        Ok(Self::from_nodes(nodes))
    }

    fn from_snapshot_nodes(nodes: Vec<CandidateNodeSnapshot>) -> Self {
        Self::from_nodes(
            nodes
                .into_iter()
                .map(|node| CandidateNode {
                    id: node.id,
                    file_node_id: node.file_node_id,
                    serialized_name_ascii_lower: node.serialized_name.to_ascii_lowercase(),
                    serialized_name: node.serialized_name,
                    qualified_name: node.qualified_name,
                })
                .collect(),
        )
    }

    fn snapshot_nodes(&self) -> Vec<CandidateNodeSnapshot> {
        self.nodes
            .iter()
            .map(|node| CandidateNodeSnapshot {
                id: node.id,
                file_node_id: node.file_node_id,
                serialized_name: node.serialized_name.clone(),
                qualified_name: node.qualified_name.clone(),
            })
            .collect()
    }

    fn from_nodes(nodes: Vec<CandidateNode>) -> Self {
        let mut index = CandidateIndex {
            nodes,
            ..CandidateIndex::default()
        };
        for (offset, node) in index.nodes.iter().enumerate() {
            index.node_offset_by_id.insert(node.id, offset);
            index
                .exact_map
                .entry(node.serialized_name.clone())
                .or_default()
                .push(offset);
            if let Some(tail) = tail_component(&node.serialized_name) {
                index
                    .suffix_map_ascii_lower
                    .entry(tail.to_ascii_lowercase())
                    .or_default()
                    .push(offset);
            }
        }
        index
    }

    #[cfg(test)]
    fn find_same_file(&self, file_id: Option<i64>, name: &str) -> Option<i64> {
        let name_ascii_lower = name.to_ascii_lowercase();
        self.find_same_file_readonly(file_id, name, &name_ascii_lower)
    }

    fn find_same_file_readonly(
        &self,
        file_id: Option<i64>,
        name: &str,
        name_ascii_lower: &str,
    ) -> Option<i64> {
        let file_id = file_id?;
        let key = (file_id, name.to_string(), name_ascii_lower.to_string());
        self.cached_lookup(&self.same_file_cache, key, || {
            self.first_in_file(self.exact_map.get(name), file_id)
                .or_else(|| {
                    self.first_in_file(self.suffix_map_ascii_lower.get(name_ascii_lower), file_id)
                })
        })
    }

    #[cfg(test)]
    fn find_same_module(&self, module_prefix: &str, delimiter: &str, name: &str) -> Option<i64> {
        let name_ascii_lower = name.to_ascii_lowercase();
        self.find_same_module_readonly(module_prefix, delimiter, name, &name_ascii_lower)
    }

    fn find_same_module_readonly(
        &self,
        module_prefix: &str,
        delimiter: &str,
        name: &str,
        name_ascii_lower: &str,
    ) -> Option<i64> {
        let qualified_prefix = format!("{module_prefix}{delimiter}");
        let key = (
            qualified_prefix.clone(),
            name.to_string(),
            name_ascii_lower.to_string(),
        );
        self.cached_lookup(&self.same_module_cache, key, || {
            self.first_in_module(self.exact_map.get(name), &qualified_prefix)
                .or_else(|| {
                    self.first_in_module(
                        self.suffix_map_ascii_lower.get(name_ascii_lower),
                        &qualified_prefix,
                    )
                })
        })
    }

    #[cfg(test)]
    fn find_global_unique(&self, name: &str) -> Option<i64> {
        let name_ascii_lower = name.to_ascii_lowercase();
        self.find_global_unique_readonly(name, &name_ascii_lower)
    }

    fn find_global_unique_readonly(&self, name: &str, name_ascii_lower: &str) -> Option<i64> {
        let key = (name.to_string(), name_ascii_lower.to_string());
        self.cached_lookup(&self.global_unique_cache, key, || {
            if let Some(exact) = self.exact_map.get(name) {
                if exact.len() == 1 {
                    return Some(self.nodes[exact[0]].id);
                }
                return None;
            }
            if let Some(suffix) = self.suffix_map_ascii_lower.get(name_ascii_lower)
                && suffix.len() == 1
            {
                return Some(self.nodes[suffix[0]].id);
            }
            None
        })
    }

    #[cfg(test)]
    fn find_fuzzy(&self, name: &str) -> Option<i64> {
        let name_ascii_lower = name.to_ascii_lowercase();
        self.find_fuzzy_readonly(name, &name_ascii_lower)
    }

    fn find_fuzzy_readonly(&self, name: &str, name_ascii_lower: &str) -> Option<i64> {
        let key = (name.to_string(), name_ascii_lower.to_string());
        self.cached_lookup(&self.fuzzy_cache, key, || {
            if let Some(exact) = self.exact_map.get(name)
                && let Some(&idx) = exact.first()
            {
                return Some(self.nodes[idx].id);
            }

            if let Some(suffix) = self.suffix_map_ascii_lower.get(name_ascii_lower)
                && let Some(&idx) = suffix.first()
            {
                return Some(self.nodes[idx].id);
            }

            self.nodes
                .iter()
                .find(|node| node.serialized_name_ascii_lower.contains(name_ascii_lower))
                .map(|node| node.id)
        })
    }

    fn top_matches_readonly(&self, name: &str, name_ascii_lower: &str, limit: usize) -> Vec<i64> {
        let mut out = Vec::with_capacity(limit);
        let mut seen = HashSet::with_capacity(limit.saturating_mul(2));
        if let Some(exact) = self.exact_map.get(name) {
            for &idx in exact.iter().take(limit) {
                let candidate = self.nodes[idx].id;
                if seen.insert(candidate) {
                    out.push(candidate);
                }
            }
        }
        if out.len() < limit
            && let Some(suffix) = self.suffix_map_ascii_lower.get(name_ascii_lower)
        {
            for &idx in suffix {
                let candidate = self.nodes[idx].id;
                if seen.insert(candidate) {
                    out.push(candidate);
                }
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }

    fn is_same_file_candidate(&self, candidate_id: i64, caller_file_id: Option<i64>) -> bool {
        let Some(caller_file_id) = caller_file_id else {
            return false;
        };
        self.node_offset_by_id
            .get(&candidate_id)
            .and_then(|offset| self.nodes.get(*offset))
            .is_some_and(|node| node.file_node_id == Some(caller_file_id))
    }

    fn first_in_file(&self, candidates: Option<&Vec<usize>>, file_id: i64) -> Option<i64> {
        candidates.and_then(|candidates| {
            candidates.iter().find_map(|idx| {
                let node = &self.nodes[*idx];
                (node.file_node_id == Some(file_id)).then_some(node.id)
            })
        })
    }

    fn first_in_module(&self, candidates: Option<&Vec<usize>>, prefix: &str) -> Option<i64> {
        candidates.and_then(|candidates| {
            candidates.iter().find_map(|idx| {
                let node = &self.nodes[*idx];
                node.qualified_name
                    .as_deref()
                    .is_some_and(|qualified| qualified.starts_with(prefix))
                    .then_some(node.id)
            })
        })
    }

    fn cached_lookup<K, F>(
        &self,
        cache: &RwLock<HashMap<K, Option<i64>>>,
        key: K,
        compute: F,
    ) -> Option<i64>
    where
        K: Clone + Eq + std::hash::Hash,
        F: FnOnce() -> Option<i64>,
    {
        if let Some(cached) = cache
            .read()
            .expect("candidate lookup cache poisoned")
            .get(&key)
            .copied()
        {
            return cached;
        }
        let resolved = compute();
        cache
            .write()
            .expect("candidate lookup cache poisoned")
            .entry(key)
            .or_insert(resolved);
        resolved
    }
}

fn tail_component(serialized_name: &str) -> Option<&str> {
    let dot_idx = serialized_name.rfind('.');
    let colon_idx = serialized_name.rfind("::");
    let start = match (dot_idx, colon_idx) {
        (Some(dot), Some(colon)) => {
            if dot > colon {
                dot + 1
            } else {
                colon + 2
            }
        }
        (Some(dot), None) => dot + 1,
        (None, Some(colon)) => colon + 2,
        (None, None) => return None,
    };
    let tail = &serialized_name[start..];
    if tail.is_empty() { None } else { Some(tail) }
}

#[cfg(test)]
#[allow(dead_code)]
fn is_same_file_candidate(
    conn: &rusqlite::Connection,
    candidate_id: i64,
    caller_file_id: Option<i64>,
) -> Result<bool> {
    let Some(caller_file_id) = caller_file_id else {
        return Ok(false);
    };
    let candidate_file_id: Option<i64> = conn
        .query_row(
            "SELECT file_node_id FROM node WHERE id = ?1",
            params![candidate_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    Ok(candidate_file_id.is_some() && candidate_file_id == Some(caller_file_id))
}

#[cfg(test)]
#[allow(dead_code)]
fn name_patterns(name: &str) -> (String, String, String) {
    (
        name.to_string(),
        format!("%.{}", name),
        format!("%::{}", name),
    )
}

fn import_name_candidates(target_name: &str, legacy_mode: bool) -> Vec<String> {
    let mut candidates = Vec::new();
    let raw = target_name.trim();
    push_unique(&mut candidates, raw.to_string());
    if legacy_mode {
        return candidates;
    }

    if let Some((lhs, rhs)) = raw.split_once(" as ") {
        push_unique(&mut candidates, lhs.trim().to_string());
        push_unique(&mut candidates, rhs.trim().to_string());
    }

    if raw.ends_with(".*") {
        push_unique(&mut candidates, raw.trim_end_matches(".*").to_string());
    }

    for delimiter in ["::", ".", "/"] {
        if let Some((_, tail)) = raw.rsplit_once(delimiter) {
            push_unique(&mut candidates, tail.trim().to_string());
        }
    }

    if let (Some(start), Some(end)) = (raw.find('{'), raw.rfind('}'))
        && start < end
    {
        let body = &raw[start + 1..end];
        for part in body.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((lhs, rhs)) = part.split_once(" as ") {
                push_unique(&mut candidates, lhs.trim().to_string());
                push_unique(&mut candidates, rhs.trim().to_string());
            } else {
                push_unique(&mut candidates, part.to_string());
            }
        }
    }

    candidates
}

fn module_prefix(qualified: &str) -> Option<(String, &'static str)> {
    if let Some(idx) = qualified.rfind("::") {
        return Some((qualified[..idx].to_string(), "::"));
    }
    if let Some(idx) = qualified.rfind('.') {
        return Some((qualified[..idx].to_string(), "."));
    }
    None
}

#[cfg(test)]
#[allow(dead_code)]
fn find_same_file(
    conn: &rusqlite::Connection,
    kind_clause: &str,
    file_id: Option<i64>,
    exact: &str,
    suffix_dot: &str,
    suffix_colon: &str,
    lookup_cache: &mut ResolutionLookupCache,
) -> Result<Option<i64>> {
    let Some(file_id) = file_id else {
        return Ok(None);
    };
    let cache_key = (kind_clause.to_string(), file_id, exact.to_string());
    if let Some(cached) = lookup_cache.same_file_lookup.get(&cache_key) {
        return Ok(*cached);
    }

    let exact_query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND file_node_id = ?1
         AND serialized_name = ?2
         ORDER BY start_line LIMIT 1",
        kind_clause
    );
    let resolved = if let Some(id) = conn
        .query_row(&exact_query, params![file_id, exact], |row| row.get(0))
        .optional()?
    {
        Some(id)
    } else {
        let suffix_query = format!(
            "SELECT id FROM node
             WHERE kind IN ({})
             AND file_node_id = ?1
             AND (serialized_name LIKE ?2 OR serialized_name LIKE ?3)
             ORDER BY start_line LIMIT 1",
            kind_clause
        );
        conn.query_row(
            &suffix_query,
            params![file_id, suffix_dot, suffix_colon],
            |row| row.get(0),
        )
        .optional()?
    };
    lookup_cache.same_file_lookup.insert(cache_key, resolved);
    Ok(resolved)
}

#[cfg(test)]
#[allow(dead_code)]
fn find_same_module(
    conn: &rusqlite::Connection,
    kind_clause: &str,
    module_prefix: &str,
    delimiter: &str,
    exact: &str,
    suffix_dot: &str,
    suffix_colon: &str,
    lookup_cache: &mut ResolutionLookupCache,
) -> Result<Option<i64>> {
    let pattern = format!("{}{}%", module_prefix, delimiter);
    let cache_key = (kind_clause.to_string(), pattern.clone(), exact.to_string());
    if let Some(cached) = lookup_cache.same_module_lookup.get(&cache_key) {
        return Ok(*cached);
    }

    let exact_query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND qualified_name LIKE ?1
         AND serialized_name = ?2
         ORDER BY start_line LIMIT 1",
        kind_clause
    );
    let resolved = if let Some(id) = conn
        .query_row(&exact_query, params![pattern, exact], |row| row.get(0))
        .optional()?
    {
        Some(id)
    } else {
        let suffix_query = format!(
            "SELECT id FROM node
             WHERE kind IN ({})
             AND qualified_name LIKE ?1
             AND (serialized_name LIKE ?2 OR serialized_name LIKE ?3)
             ORDER BY start_line LIMIT 1",
            kind_clause
        );
        conn.query_row(
            &suffix_query,
            params![pattern, suffix_dot, suffix_colon],
            |row| row.get(0),
        )
        .optional()?
    };
    lookup_cache.same_module_lookup.insert(cache_key, resolved);
    Ok(resolved)
}

#[cfg(test)]
#[allow(dead_code)]
fn find_global_unique(
    conn: &rusqlite::Connection,
    kind_clause: &str,
    exact: &str,
    suffix_dot: &str,
    suffix_colon: &str,
    lookup_cache: &mut ResolutionLookupCache,
) -> Result<Option<i64>> {
    let cache_key = (kind_clause.to_string(), exact.to_string());
    if let Some(cached) = lookup_cache.global_unique_lookup.get(&cache_key) {
        return Ok(*cached);
    }

    let exact_count_query = format!(
        "SELECT COUNT(*) FROM node
         WHERE kind IN ({})
         AND serialized_name = ?1",
        kind_clause
    );
    let exact_count: i64 = conn.query_row(&exact_count_query, params![exact], |row| row.get(0))?;
    let resolved = if exact_count == 1 {
        let exact_query = format!(
            "SELECT id FROM node
             WHERE kind IN ({})
             AND serialized_name = ?1
             LIMIT 1",
            kind_clause
        );
        conn.query_row(&exact_query, params![exact], |row| row.get(0))
            .optional()?
    } else if exact_count > 1 {
        None
    } else {
        let suffix_count_query = format!(
            "SELECT COUNT(*) FROM node
             WHERE kind IN ({})
             AND (serialized_name LIKE ?1 OR serialized_name LIKE ?2)",
            kind_clause
        );
        let suffix_count: i64 = conn.query_row(
            &suffix_count_query,
            params![suffix_dot, suffix_colon],
            |row| row.get(0),
        )?;
        if suffix_count != 1 {
            None
        } else {
            let suffix_query = format!(
                "SELECT id FROM node
                 WHERE kind IN ({})
                 AND (serialized_name LIKE ?1 OR serialized_name LIKE ?2)
                 LIMIT 1",
                kind_clause
            );
            conn.query_row(&suffix_query, params![suffix_dot, suffix_colon], |row| {
                row.get(0)
            })
            .optional()?
        }
    };
    lookup_cache
        .global_unique_lookup
        .insert(cache_key, resolved);
    Ok(resolved)
}

#[cfg(test)]
#[allow(dead_code)]
fn find_fuzzy(
    conn: &rusqlite::Connection,
    kind_clause: &str,
    exact: &str,
    suffix_dot: &str,
    suffix_colon: &str,
) -> Result<Option<i64>> {
    let exact_query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND serialized_name = ?1
         ORDER BY start_line LIMIT 1",
        kind_clause
    );
    if let Some(id) = conn
        .query_row(&exact_query, params![exact], |row| row.get(0))
        .optional()?
    {
        return Ok(Some(id));
    }

    let suffix_query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND (serialized_name LIKE ?1 OR serialized_name LIKE ?2)
         ORDER BY start_line LIMIT 1",
        kind_clause
    );
    if let Some(id) = conn
        .query_row(&suffix_query, params![suffix_dot, suffix_colon], |row| {
            row.get(0)
        })
        .optional()?
    {
        return Ok(Some(id));
    }

    let fuzzy = format!("%{}%", exact);
    let query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND serialized_name LIKE ?1
         ORDER BY start_line LIMIT 1",
        kind_clause
    );
    conn.query_row(&query, params![fuzzy], |row| row.get(0))
        .optional()
        .map_err(Into::into)
}

#[cfg(test)]
#[allow(dead_code)]
fn collect_candidate_pool(
    conn: &rusqlite::Connection,
    kind_clause: &str,
    names: &[String],
    out: &mut Vec<i64>,
    limit: usize,
) -> Result<()> {
    if out.len() >= limit {
        return Ok(());
    }
    for name in names {
        let (exact, suffix_dot, suffix_colon) = name_patterns(name);
        let top = find_top_matches(conn, kind_clause, &exact, &suffix_dot, &suffix_colon, 3)?;
        for id in top {
            record_candidate(out, id);
            if out.len() >= limit {
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(dead_code)]
fn find_top_matches(
    conn: &rusqlite::Connection,
    kind_clause: &str,
    exact: &str,
    suffix_dot: &str,
    suffix_colon: &str,
    limit: usize,
) -> Result<Vec<i64>> {
    let exact_query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND serialized_name = ?1
         ORDER BY start_line
         LIMIT {}",
        kind_clause, limit
    );
    let mut out = Vec::with_capacity(limit);
    {
        let mut stmt = conn.prepare(&exact_query)?;
        let rows = stmt.query_map(params![exact], |row| row.get(0))?;
        for row in rows {
            out.push(row?);
        }
    }
    if out.len() >= limit {
        return Ok(out);
    }

    let remaining = limit - out.len();
    let suffix_query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND (serialized_name LIKE ?1 OR serialized_name LIKE ?2)
         ORDER BY start_line
         LIMIT {}",
        kind_clause, remaining
    );
    let mut stmt = conn.prepare(&suffix_query)?;
    let rows = stmt.query_map(params![suffix_dot, suffix_colon], |row| row.get(0))?;
    for row in rows {
        let candidate = row?;
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    Ok(out)
}

fn candidate_json(candidates: &[i64]) -> Result<Option<String>> {
    if candidates.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(candidates)?))
}

#[cfg(test)]
fn record_candidate(candidates: &mut Vec<i64>, candidate: i64) {
    if !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

fn consider_selected(selected: &mut Option<(i64, f32)>, candidate_id: i64, confidence: f32) {
    let replace = match *selected {
        Some((_, existing_confidence)) => confidence > existing_confidence,
        None => true,
    };
    if replace {
        *selected = Some((candidate_id, confidence));
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn kind_clause(kinds: &[i32]) -> String {
    kinds
        .iter()
        .map(|k| k.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn common_unqualified_call_names() -> &'static [&'static str] {
    &[
        "add",
        "any",
        "clear",
        "clone",
        "commit",
        "collect",
        "copied",
        "dedup",
        "default",
        "execute",
        "extend",
        "from",
        "is_empty",
        "insert",
        "iter",
        "len",
        "map",
        "map_err",
        "once",
        "ok",
        "pop",
        "push",
        "remove",
        "sort",
        "sort_by",
        "sort_by_key",
        "truncate",
    ]
}

fn is_common_unqualified_call_name(name: &str) -> bool {
    if name.contains("::") || name.contains('.') {
        return false;
    }
    let normalized = name.to_ascii_lowercase();
    common_unqualified_call_names().contains(&normalized.as_str())
}

fn should_keep_common_call_resolution(
    target_name: &str,
    confidence: f32,
    callsite_identity: Option<&str>,
) -> bool {
    if !is_common_unqualified_call_name(target_name) {
        return true;
    }

    let certainty = ResolutionCertainty::from_confidence(Some(confidence));
    matches!(certainty, Some(ResolutionCertainty::Certain)) && callsite_identity.is_some()
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

fn duration_ms_u64(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, params};
    use std::collections::HashSet;
    use std::time::Instant;
    use tempfile::tempdir;

    fn create_node_table(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE node (
                id INTEGER PRIMARY KEY,
                kind INTEGER NOT NULL,
                serialized_name TEXT NOT NULL,
                qualified_name TEXT,
                file_node_id INTEGER,
                start_line INTEGER
            );",
        )?;
        Ok(())
    }

    #[test]
    fn test_prepared_resolution_state_rebuilds_snapshot_when_semantic_mode_changes() -> Result<()> {
        let mut storage = Storage::new_in_memory()?;
        storage.insert_nodes_batch(&[
            codestory_core::Node {
                id: codestory_core::NodeId(1),
                kind: NodeKind::FILE,
                serialized_name: "/repo/lib.rs".to_string(),
                ..Default::default()
            },
            codestory_core::Node {
                id: codestory_core::NodeId(2),
                kind: NodeKind::FUNCTION,
                serialized_name: "target".to_string(),
                qualified_name: Some("pkg::target".to_string()),
                file_node_id: Some(codestory_core::NodeId(1)),
                ..Default::default()
            },
        ])?;

        let stale_snapshot = ResolutionSupportSnapshot {
            enable_semantic: false,
            call_candidates: Vec::new(),
            import_candidates: Vec::new(),
            call_semantic_nodes: Vec::new(),
            import_semantic_nodes: Vec::new(),
            override_members: Vec::new(),
            override_inheritance: Vec::new(),
            override_inheritance_by_name: Vec::new(),
            node_names: Vec::new(),
        };
        storage.put_resolution_support_snapshot(
            RESOLUTION_SUPPORT_SNAPSHOT_VERSION,
            &serde_json::to_vec(&stale_snapshot)?,
        )?;

        let flags = ResolutionFlags {
            legacy_mode: false,
            enable_semantic: true,
            store_candidates: false,
            parallel_compute: false,
        };
        let mut telemetry = ResolutionPhaseTelemetry::default();
        let prepared = PreparedResolutionState::load(&storage, flags, &mut telemetry)?;

        assert!(
            !telemetry.support_snapshot_hit,
            "semantic mode mismatch should force a snapshot rebuild"
        );
        assert!(
            !prepared.call_semantic_index.snapshot_nodes().is_empty(),
            "rebuilt state should include semantic candidates from storage"
        );

        let rebuilt_blob = storage
            .get_resolution_support_snapshot(RESOLUTION_SUPPORT_SNAPSHOT_VERSION)?
            .expect("rebuilt snapshot");
        let rebuilt: ResolutionSupportSnapshot = serde_json::from_slice(&rebuilt_blob)?;
        assert!(rebuilt.enable_semantic);
        assert!(!rebuilt.call_semantic_nodes.is_empty());

        Ok(())
    }

    #[test]
    fn test_common_unqualified_call_names_include_clone_like_noise() {
        assert!(is_common_unqualified_call_name("clone"));
        assert!(is_common_unqualified_call_name("len"));
        assert!(is_common_unqualified_call_name("once"));
        assert!(is_common_unqualified_call_name("Ok"));
        assert!(!is_common_unqualified_call_name(
            "WorkspaceIndexer::run_incremental"
        ));
    }

    #[test]
    fn test_common_call_resolution_requires_certain_confidence_and_callsite() {
        assert!(!should_keep_common_call_resolution(
            "clone",
            0.80,
            Some("1:2:3:4")
        ));
        assert!(!should_keep_common_call_resolution("clone", 0.95, None));
        assert!(should_keep_common_call_resolution(
            "clone",
            ResolutionCertainty::CERTAIN_MIN,
            Some("1:2:3:4")
        ));
        for name in [
            "once", "execute", "Ok", "commit", "default", "iter", "copied", "from",
        ] {
            assert!(
                !should_keep_common_call_resolution(name, 0.95, None),
                "name={name} should be suppressed without a strict callsite"
            );
            assert!(
                should_keep_common_call_resolution(
                    name,
                    ResolutionCertainty::CERTAIN_MIN,
                    Some("1:2:3:4")
                ),
                "name={name} should be kept when certain and tied to a callsite"
            );
        }
        assert!(should_keep_common_call_resolution(
            "numbered_placeholders",
            0.80,
            None
        ));
    }

    #[test]
    fn test_semantic_candidates_for_rows_dedupe_identical_requests() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        let index = SemanticCandidateIndex::load(&conn, &[NodeKind::FUNCTION as i32])?;
        let flags = ResolutionFlags {
            legacy_mode: false,
            enable_semantic: true,
            store_candidates: false,
            parallel_compute: false,
        };
        let pass = ResolutionPass {
            flags,
            policy: ResolutionPolicy::for_flags(flags),
            semantic_resolvers: SemanticResolverRegistry::new(true),
        };
        let rows = vec![
            (
                1_i64,
                Some(100_i64),
                Some("pkg::core::caller".to_string()),
                "caller".to_string(),
                "target".to_string(),
                Some("/repo/lib.rs".to_string()),
                Some("1:2:3:4".to_string()),
            ),
            (
                2_i64,
                Some(100_i64),
                Some("pkg::core::caller".to_string()),
                "caller".to_string(),
                "target".to_string(),
                Some("/repo/lib.rs".to_string()),
                Some("1:2:3:4".to_string()),
            ),
        ];

        let (candidates, stats) =
            pass.semantic_candidates_for_rows(&index, &rows, EdgeKind::CALL)?;
        assert_eq!(candidates.len(), 2);
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.unique_requests, 1);
        assert_eq!(stats.skipped_requests, 0);
        Ok(())
    }

    #[test]
    fn test_common_call_without_callsite_skips_semantic_requests() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        let index = SemanticCandidateIndex::load(&conn, &[NodeKind::FUNCTION as i32])?;
        let flags = ResolutionFlags {
            legacy_mode: false,
            enable_semantic: true,
            store_candidates: false,
            parallel_compute: false,
        };
        let pass = ResolutionPass {
            flags,
            policy: ResolutionPolicy::for_flags(flags),
            semantic_resolvers: SemanticResolverRegistry::new(true),
        };
        let rows = vec![(
            1_i64,
            Some(100_i64),
            Some("pkg::core::caller".to_string()),
            "caller".to_string(),
            "clone".to_string(),
            Some("/repo/lib.rs".to_string()),
            None,
        )];

        let (candidates, stats) =
            pass.semantic_candidates_for_rows(&index, &rows, EdgeKind::CALL)?;
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].is_empty());
        assert_eq!(stats.total_requests, 1);
        assert_eq!(stats.unique_requests, 0);
        assert_eq!(stats.skipped_requests, 1);
        Ok(())
    }

    #[test]
    fn test_common_call_same_module_candidate_is_not_selected_but_is_retained_for_candidates()
    -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                10_i64,
                NodeKind::FUNCTION as i32,
                "clone",
                "pkg::core::clone",
                100_i64,
                1_i64
            ],
        )?;
        let index = CandidateIndex::load(&conn, &[NodeKind::FUNCTION as i32])?;
        let flags = ResolutionFlags {
            legacy_mode: false,
            enable_semantic: false,
            store_candidates: true,
            parallel_compute: false,
        };
        let pass = ResolutionPass {
            flags,
            policy: ResolutionPolicy::for_flags(flags),
            semantic_resolvers: SemanticResolverRegistry::new(false),
        };
        let row = (
            1_i64,
            Some(100_i64),
            Some("pkg::core::caller".to_string()),
            "caller".to_string(),
            "clone".to_string(),
            Some("/repo/lib.rs".to_string()),
            Some("1:2:3:4".to_string()),
        );

        let computed = candidate_selection::compute_call_resolution(&pass, &index, &row, &[])?;
        assert_eq!(computed.strategy, None);
        assert_eq!(computed.update.resolved_target_node_id, None);
        let payload = computed
            .update
            .candidate_payload
            .as_deref()
            .expect("candidate payload");
        let candidates: Vec<i64> = serde_json::from_str(payload)?;
        assert_eq!(candidates, vec![10_i64]);
        Ok(())
    }

    #[test]
    fn test_common_call_certain_semantic_candidate_still_resolves() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        let index = CandidateIndex::load(&conn, &[NodeKind::FUNCTION as i32])?;
        let flags = ResolutionFlags {
            legacy_mode: false,
            enable_semantic: true,
            store_candidates: false,
            parallel_compute: false,
        };
        let pass = ResolutionPass {
            flags,
            policy: ResolutionPolicy::for_flags(flags),
            semantic_resolvers: SemanticResolverRegistry::new(true),
        };
        let row = (
            2_i64,
            Some(100_i64),
            Some("pkg::core::caller".to_string()),
            "caller".to_string(),
            "clone".to_string(),
            Some("/repo/lib.rs".to_string()),
            Some("1:2:3:4".to_string()),
        );
        let semantic_candidates = vec![SemanticResolutionCandidate {
            target_node_id: 77_i64,
            confidence: ResolutionCertainty::CERTAIN_MIN,
        }];

        let computed = candidate_selection::compute_call_resolution(
            &pass,
            &index,
            &row,
            &semantic_candidates,
        )?;
        assert_eq!(
            computed.strategy,
            Some(ResolutionStrategy::CallSemanticFallback)
        );
        assert_eq!(computed.update.resolved_target_node_id, Some(77_i64));
        Ok(())
    }

    #[test]
    fn test_semantic_language_bucket_matrix() {
        let expected = [
            ("a.c", Some("c")),
            ("a.cpp", Some("cpp")),
            ("a.h", Some("cpp")),
            ("a.hh", Some("cpp")),
            ("a.java", Some("java")),
            ("a.js", Some("javascript")),
            ("a.jsx", Some("javascript")),
            ("a.pyi", Some("python")),
            ("a.rs", Some("rust")),
            ("a.ts", Some("typescript")),
            ("a.tsx", Some("typescript")),
            ("a.mts", Some("typescript")),
            ("a.unknown", None),
        ];
        for (path, language) in expected {
            assert_eq!(
                semantic_language_bucket(Some(path)),
                language,
                "path={path}"
            );
        }
    }

    #[test]
    fn test_scope_filters_unresolved_edges_and_counts() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE node (
                id INTEGER PRIMARY KEY,
                kind INTEGER NOT NULL,
                serialized_name TEXT NOT NULL,
                qualified_name TEXT,
                file_node_id INTEGER,
                start_line INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE edge (
                id INTEGER PRIMARY KEY,
                kind INTEGER NOT NULL,
                source_node_id INTEGER NOT NULL,
                target_node_id INTEGER NOT NULL,
                resolved_target_node_id INTEGER,
                callsite_identity TEXT
            );",
        )?;

        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, NULL, NULL, 1)",
            params![100_i64, NodeKind::FILE as i32, "/repo/a.rs"],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, NULL, NULL, 1)",
            params![200_i64, NodeKind::FILE as i32, "/repo/b.rs"],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                1_i64,
                NodeKind::FUNCTION as i32,
                "caller_a",
                "mod_a::caller",
                100_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                2_i64,
                NodeKind::FUNCTION as i32,
                "caller_b",
                "mod_b::caller",
                200_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                10_i64,
                NodeKind::FUNCTION as i32,
                "target_fn",
                "mod::target_fn",
                100_i64
            ],
        )?;

        conn.execute(
            "INSERT INTO edge (id, kind, source_node_id, target_node_id, resolved_target_node_id, callsite_identity)
             VALUES (?1, ?2, ?3, ?4, NULL, NULL)",
            params![1000_i64, EdgeKind::CALL as i32, 1_i64, 10_i64],
        )?;
        conn.execute(
            "INSERT INTO edge (id, kind, source_node_id, target_node_id, resolved_target_node_id, callsite_identity)
             VALUES (?1, ?2, ?3, ?4, NULL, NULL)",
            params![1001_i64, EdgeKind::CALL as i32, 2_i64, 10_i64],
        )?;

        let scope = HashSet::from([100_i64]);
        let scoped_context = ScopeCallerContext::prepare(&conn, Some(&scope))?;
        let unscoped_context = ScopeCallerContext::prepare(&conn, None)?;
        let scoped_rows = sql::unresolved_edges(&conn, EdgeKind::CALL, &scoped_context)?;
        assert_eq!(scoped_rows.len(), 1);
        assert_eq!(scoped_rows[0].0, 1000_i64);
        assert_eq!(scoped_rows[0].1, Some(100_i64));

        let all_rows = sql::unresolved_edges(&conn, EdgeKind::CALL, &unscoped_context)?;
        assert_eq!(all_rows.len(), 2);

        let scoped_count =
            ResolutionPass::count_unresolved_on_conn(&conn, EdgeKind::CALL, &scoped_context)?;
        let all_count =
            ResolutionPass::count_unresolved_on_conn(&conn, EdgeKind::CALL, &unscoped_context)?;
        assert_eq!(scoped_count, 1);
        assert_eq!(all_count, 2);
        Ok(())
    }

    #[test]
    fn test_exact_lookup_cache_reuses_same_file_key() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE node (
                id INTEGER PRIMARY KEY,
                kind INTEGER NOT NULL,
                serialized_name TEXT NOT NULL,
                file_node_id INTEGER,
                start_line INTEGER NOT NULL DEFAULT 0
            );",
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![77_i64, NodeKind::FUNCTION as i32, "foo", 555_i64, 1_i64],
        )?;

        let kind_clause = kind_clause(&[NodeKind::FUNCTION as i32]);
        let (exact, suffix_dot, suffix_colon) = name_patterns("foo");
        let mut lookup_cache = ResolutionLookupCache::default();

        let first = find_same_file(
            &conn,
            &kind_clause,
            Some(555_i64),
            &exact,
            &suffix_dot,
            &suffix_colon,
            &mut lookup_cache,
        )?;
        assert_eq!(first, Some(77_i64));
        assert_eq!(lookup_cache.same_file_lookup.len(), 1);

        let second = find_same_file(
            &conn,
            &kind_clause,
            Some(555_i64),
            &exact,
            &suffix_dot,
            &suffix_colon,
            &mut lookup_cache,
        )?;
        assert_eq!(second, Some(77_i64));
        assert_eq!(lookup_cache.same_file_lookup.len(), 1);
        Ok(())
    }

    #[test]
    fn test_candidate_index_same_file_exact_beats_suffix() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                10_i64,
                NodeKind::FUNCTION as i32,
                "target",
                "pkg::target",
                101_i64,
                10_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                11_i64,
                NodeKind::FUNCTION as i32,
                "pkg::target",
                "pkg::nested::target",
                101_i64,
                5_i64
            ],
        )?;

        let index = CandidateIndex::load(&conn, &[NodeKind::FUNCTION as i32])?;
        assert_eq!(index.find_same_file(Some(101_i64), "target"), Some(10_i64));
        Ok(())
    }

    #[test]
    fn test_candidate_index_lookup_caches_stay_stable_across_repeated_reads() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                10_i64,
                NodeKind::FUNCTION as i32,
                "build",
                "pkg::core::build",
                101_i64,
                1_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                11_i64,
                NodeKind::MODULE as i32,
                "UniqueThing",
                "pkg::UniqueThing",
                102_i64,
                1_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                12_i64,
                NodeKind::MODULE as i32,
                "pkg::ContainsOnlyTarget",
                "pkg::ContainsOnlyTarget",
                103_i64,
                1_i64
            ],
        )?;

        let index =
            CandidateIndex::load(&conn, &[NodeKind::FUNCTION as i32, NodeKind::MODULE as i32])?;
        assert_eq!(
            index.find_same_file_readonly(Some(101_i64), "build", "build"),
            Some(10_i64)
        );
        assert_eq!(
            index.find_same_file_readonly(Some(101_i64), "build", "build"),
            Some(10_i64)
        );
        assert_eq!(
            index.find_same_module_readonly("pkg::core", "::", "build", "build"),
            Some(10_i64)
        );
        assert_eq!(
            index.find_same_module_readonly("pkg::core", "::", "build", "build"),
            Some(10_i64)
        );
        assert_eq!(
            index.find_global_unique_readonly("UniqueThing", "uniquething"),
            Some(11_i64)
        );
        assert_eq!(
            index.find_global_unique_readonly("UniqueThing", "uniquething"),
            Some(11_i64)
        );
        assert_eq!(
            index.find_fuzzy_readonly("ContainsOnlyTarget", "containsonlytarget"),
            Some(12_i64)
        );
        assert_eq!(
            index.find_fuzzy_readonly("ContainsOnlyTarget", "containsonlytarget"),
            Some(12_i64)
        );
        assert_eq!(
            index
                .same_file_cache
                .read()
                .expect("same-file cache readable")
                .len(),
            1
        );
        assert_eq!(
            index
                .same_module_cache
                .read()
                .expect("same-module cache readable")
                .len(),
            1
        );
        assert_eq!(
            index
                .global_unique_cache
                .read()
                .expect("global cache readable")
                .len(),
            1
        );
        assert_eq!(
            index
                .fuzzy_cache
                .read()
                .expect("fuzzy cache readable")
                .len(),
            1
        );
        Ok(())
    }

    #[test]
    fn test_candidate_index_same_module_and_suffix_resolution() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                20_i64,
                NodeKind::FUNCTION as i32,
                "build",
                "pkg::core::build",
                201_i64,
                40_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                21_i64,
                NodeKind::FUNCTION as i32,
                "pkg::helper",
                "pkg::core::helper",
                201_i64,
                20_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                22_i64,
                NodeKind::FUNCTION as i32,
                "build_dot",
                "pkg.core.build_dot",
                201_i64,
                30_i64
            ],
        )?;

        let index = CandidateIndex::load(&conn, &[NodeKind::FUNCTION as i32])?;
        assert_eq!(
            index.find_same_module("pkg::core", "::", "build"),
            Some(20_i64)
        );
        assert_eq!(
            index.find_same_module("pkg::core", "::", "helper"),
            Some(21_i64)
        );
        assert_eq!(
            index.find_same_module("pkg.core", ".", "build_dot"),
            Some(22_i64)
        );
        Ok(())
    }

    #[test]
    fn test_candidate_index_global_unique_and_fuzzy_order() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
            params![
                30_i64,
                NodeKind::MODULE as i32,
                "UniqueThing",
                "pkg::UniqueThing",
                1_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
            params![
                31_i64,
                NodeKind::MODULE as i32,
                "pkg::AliasThing",
                "pkg::AliasThing",
                2_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
            params![
                32_i64,
                NodeKind::MODULE as i32,
                "other::AliasThing",
                "other::AliasThing",
                3_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
            params![
                33_i64,
                NodeKind::MODULE as i32,
                "pkg::ContainsOnlyTarget",
                "pkg::ContainsOnlyTarget",
                4_i64
            ],
        )?;

        let index = CandidateIndex::load(&conn, &[NodeKind::MODULE as i32])?;
        assert_eq!(index.find_global_unique("UniqueThing"), Some(30_i64));
        assert_eq!(index.find_global_unique("AliasThing"), None);
        assert_eq!(index.find_fuzzy("ContainsOnlyTarget"), Some(33_i64));
        assert_eq!(index.find_fuzzy("containsonly"), Some(33_i64));
        Ok(())
    }

    #[test]
    fn test_candidate_index_same_file_candidate_detection() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                41_i64,
                NodeKind::MODULE as i32,
                "pkg::same",
                "pkg::same",
                900_i64,
                1_i64
            ],
        )?;
        let index = CandidateIndex::load(&conn, &[NodeKind::MODULE as i32])?;
        assert!(index.is_same_file_candidate(41_i64, Some(900_i64)));
        assert!(!index.is_same_file_candidate(41_i64, Some(901_i64)));
        assert!(!index.is_same_file_candidate(41_i64, None));
        Ok(())
    }

    #[test]
    fn test_scoped_cleanup_only_mutates_scoped_callers() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE node (
                id INTEGER PRIMARY KEY,
                kind INTEGER NOT NULL,
                serialized_name TEXT NOT NULL,
                qualified_name TEXT,
                file_node_id INTEGER,
                start_line INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE edge (
                id INTEGER PRIMARY KEY,
                kind INTEGER NOT NULL,
                source_node_id INTEGER NOT NULL,
                target_node_id INTEGER NOT NULL,
                resolved_target_node_id INTEGER,
                confidence REAL,
                certainty TEXT
            );",
        )?;

        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                1_i64,
                NodeKind::FUNCTION as i32,
                "caller_one",
                "pkg::one",
                100_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                2_i64,
                NodeKind::FUNCTION as i32,
                "caller_two",
                "pkg::two",
                200_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                50_i64,
                NodeKind::FUNCTION as i32,
                "helper_target",
                "pkg::helper_target",
                100_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO edge (id, kind, source_node_id, target_node_id, resolved_target_node_id, confidence, certainty)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
            params![900_i64, EdgeKind::CALL as i32, 1_i64, 50_i64, 50_i64, 0.10_f64],
        )?;
        conn.execute(
            "INSERT INTO edge (id, kind, source_node_id, target_node_id, resolved_target_node_id, confidence, certainty)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
            params![901_i64, EdgeKind::CALL as i32, 2_i64, 50_i64, 50_i64, 0.10_f64],
        )?;

        let flags = ResolutionFlags {
            legacy_mode: false,
            enable_semantic: false,
            store_candidates: false,
            parallel_compute: false,
        };
        let policy = ResolutionPolicy::for_flags(flags);
        let scope = HashSet::from([100_i64]);
        let scope_context = ScopeCallerContext::prepare(&conn, Some(&scope))?;
        sql::cleanup_stale_call_resolutions(&conn, flags, policy, &scope_context)?;

        let scoped_target: Option<i64> = conn.query_row(
            "SELECT resolved_target_node_id FROM edge WHERE id = ?1",
            params![900_i64],
            |row| row.get(0),
        )?;
        let unscoped_target: Option<i64> = conn.query_row(
            "SELECT resolved_target_node_id FROM edge WHERE id = ?1",
            params![901_i64],
            |row| row.get(0),
        )?;
        assert_eq!(scoped_target, None);
        assert_eq!(unscoped_target, Some(50_i64));
        Ok(())
    }

    #[test]
    fn test_transaction_smoke_is_faster_than_autocommit() -> Result<()> {
        let dir = tempdir()?;
        let db_path = dir.path().join("resolution_perf_smoke.db");
        let conn = Connection::open(db_path)?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        conn.execute(
            "CREATE TABLE perf (id INTEGER PRIMARY KEY, value INTEGER NOT NULL)",
            [],
        )?;

        run_in_immediate_transaction(&conn, |tx_conn| {
            let mut stmt = tx_conn.prepare("INSERT INTO perf (id, value) VALUES (?1, 0)")?;
            for id in 1..=600_i64 {
                stmt.execute(params![id])?;
            }
            Ok(())
        })?;

        let ids: Vec<i64> = (1..=600_i64).collect();

        let no_tx_start = Instant::now();
        for id in &ids {
            conn.execute(
                "UPDATE perf SET value = value + 1 WHERE id = ?1",
                params![id],
            )?;
        }
        let no_tx_elapsed = no_tx_start.elapsed();

        conn.execute("UPDATE perf SET value = 0", [])?;

        let tx_start = Instant::now();
        run_in_immediate_transaction(&conn, |tx_conn| {
            let mut stmt = tx_conn.prepare("UPDATE perf SET value = value + 1 WHERE id = ?1")?;
            for id in &ids {
                stmt.execute(params![id])?;
            }
            Ok(())
        })?;
        let tx_elapsed = tx_start.elapsed();

        assert!(
            tx_elapsed < no_tx_elapsed,
            "expected transaction updates to beat autocommit; no_tx={:?}, tx={:?}",
            no_tx_elapsed,
            tx_elapsed
        );
        Ok(())
    }
}
