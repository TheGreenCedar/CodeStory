use crate::semantic::{
    SemanticResolutionCandidate, SemanticResolutionRequest, SemanticResolverRegistry,
};
use anyhow::Result;
use codestory_core::{EdgeKind, NodeKind, ResolutionCertainty};
use codestory_storage::Storage;
use rayon::prelude::*;
#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{params, params_from_iter, types::Value};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

type UnresolvedEdgeRow = (
    i64,
    Option<i64>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);

const SCOPED_CALLER_TABLE: &str = "resolution_scoped_caller_ids";

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

#[derive(Default, Debug)]
struct CandidateIndex {
    nodes: Vec<CandidateNode>,
    node_offset_by_id: HashMap<i64, usize>,
    exact_map: HashMap<String, Vec<usize>>,
    suffix_map_ascii_lower: HashMap<String, Vec<usize>>,
    #[cfg(test)]
    same_file_cache: HashMap<(i64, String), Option<i64>>,
    #[cfg(test)]
    same_module_cache: HashMap<(String, String), Option<i64>>,
    #[cfg(test)]
    global_unique_cache: HashMap<String, Option<i64>>,
    #[cfg(test)]
    fuzzy_cache_ascii_lower: HashMap<String, Option<i64>>,
}

#[derive(Debug, Clone)]
struct ResolvedEdgeUpdate {
    edge_id: i64,
    resolved_target_node_id: Option<i64>,
    confidence: Option<f32>,
    certainty: Option<&'static str>,
    candidate_payload: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SemanticCacheKey {
    edge_kind: EdgeKind,
    file_id: Option<i64>,
    caller_qualified: Option<String>,
    target_name: String,
    language_bucket: Option<String>,
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

#[derive(Default, Debug, Clone, Copy)]
pub struct ResolutionPhaseTelemetry {
    pub scope_prepare_ms: u64,
    pub unresolved_count_start_ms: u64,
    pub call_prepare_ms: u64,
    pub call_cleanup_ms: u64,
    pub call_unresolved_load_ms: u64,
    pub call_candidate_index_ms: u64,
    pub call_compute_ms: u64,
    pub call_apply_ms: u64,
    pub import_prepare_ms: u64,
    pub import_unresolved_load_ms: u64,
    pub import_candidate_index_ms: u64,
    pub import_compute_ms: u64,
    pub import_apply_ms: u64,
    pub unresolved_count_end_ms: u64,
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
        query.push_str(&numbered_placeholders(1, scope_ids.len()));
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
            store_candidates: env_flag("CODESTORY_RESOLUTION_STORE_CANDIDATES", !legacy_mode),
            parallel_compute: env_flag("CODESTORY_RESOLUTION_PARALLEL_COMPUTE", false),
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

            if unresolved_calls_before == 0 && unresolved_imports_before == 0 {
                return Ok(ResolutionStats {
                    unresolved_calls_before,
                    resolved_calls: 0,
                    unresolved_calls: unresolved_calls_before,
                    unresolved_imports_before,
                    resolved_imports: 0,
                    unresolved_imports: unresolved_imports_before,
                    telemetry,
                    strategy_counters: ResolutionStrategyCounters::default(),
                });
            }

            let mut strategy_counters = ResolutionStrategyCounters::default();
            let resolved_calls = self.resolve_calls_on_conn(
                conn,
                &scope_context,
                &mut telemetry,
                &mut strategy_counters,
            )?;
            let resolved_imports = self.resolve_imports_on_conn(
                conn,
                &scope_context,
                &mut telemetry,
                &mut strategy_counters,
            )?;

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
        let mut strategy_counters = ResolutionStrategyCounters::default();
        self.resolve_calls_on_conn(conn, &scope_context, &mut telemetry, &mut strategy_counters)
    }

    fn resolve_calls_on_conn(
        &self,
        conn: &rusqlite::Connection,
        scope_context: &ScopeCallerContext,
        telemetry: &mut ResolutionPhaseTelemetry,
        strategy_counters: &mut ResolutionStrategyCounters,
    ) -> Result<usize> {
        if scope_context.is_empty() {
            return Ok(0);
        }

        let prepare_started = Instant::now();
        conn.execute(
            "UPDATE edge SET resolved_source_node_id = source_node_id
             WHERE kind = ?1 AND resolved_source_node_id IS NULL",
            params![EdgeKind::CALL as i32],
        )?;
        telemetry.call_prepare_ms = telemetry
            .call_prepare_ms
            .saturating_add(duration_ms_u64(prepare_started.elapsed()));

        let cleanup_started = Instant::now();
        cleanup_stale_call_resolutions(conn, self.flags, self.policy, scope_context)?;
        telemetry.call_cleanup_ms = telemetry
            .call_cleanup_ms
            .saturating_add(duration_ms_u64(cleanup_started.elapsed()));

        let rows_started = Instant::now();
        let rows = unresolved_edges(conn, EdgeKind::CALL, scope_context)?;
        telemetry.call_unresolved_load_ms = telemetry
            .call_unresolved_load_ms
            .saturating_add(duration_ms_u64(rows_started.elapsed()));
        if rows.is_empty() {
            return Ok(0);
        }

        let candidate_started = Instant::now();
        let candidate_index = CandidateIndex::load(
            conn,
            &[
                NodeKind::FUNCTION as i32,
                NodeKind::METHOD as i32,
                NodeKind::MACRO as i32,
            ],
        )?;
        telemetry.call_candidate_index_ms = telemetry
            .call_candidate_index_ms
            .saturating_add(duration_ms_u64(candidate_started.elapsed()));

        let semantic_candidates_by_row =
            self.semantic_candidates_for_rows(conn, &rows, EdgeKind::CALL)?;

        let compute_started = Instant::now();
        let computed_results: Vec<Result<ComputedResolution>> =
            if self.flags.parallel_compute && rows.len() > 1 {
                rows.par_iter()
                    .zip(semantic_candidates_by_row.par_iter())
                    .map(|(row, semantic_candidates)| {
                        self.compute_call_resolution(&candidate_index, row, semantic_candidates)
                    })
                    .collect()
            } else {
                rows.iter()
                    .zip(semantic_candidates_by_row.iter())
                    .map(|(row, semantic_candidates)| {
                        self.compute_call_resolution(&candidate_index, row, semantic_candidates)
                    })
                    .collect()
            };
        telemetry.call_compute_ms = telemetry
            .call_compute_ms
            .saturating_add(duration_ms_u64(compute_started.elapsed()));

        let mut resolved = 0usize;
        let mut updates = Vec::with_capacity(rows.len());
        for computed in computed_results {
            let computed = computed?;
            if computed.strategy.is_some() {
                resolved += 1;
            }
            strategy_counters.record(computed.strategy);
            updates.push(computed.update);
        }

        let apply_started = Instant::now();
        apply_resolution_updates(conn, &updates)?;
        telemetry.call_apply_ms = telemetry
            .call_apply_ms
            .saturating_add(duration_ms_u64(apply_started.elapsed()));
        Ok(resolved)
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
        let mut strategy_counters = ResolutionStrategyCounters::default();
        self.resolve_imports_on_conn(conn, &scope_context, &mut telemetry, &mut strategy_counters)
    }

    fn resolve_imports_on_conn(
        &self,
        conn: &rusqlite::Connection,
        scope_context: &ScopeCallerContext,
        telemetry: &mut ResolutionPhaseTelemetry,
        strategy_counters: &mut ResolutionStrategyCounters,
    ) -> Result<usize> {
        if scope_context.is_empty() {
            return Ok(0);
        }

        let prepare_started = Instant::now();
        conn.execute(
            "UPDATE edge SET resolved_source_node_id = source_node_id
             WHERE kind = ?1 AND resolved_source_node_id IS NULL",
            params![EdgeKind::IMPORT as i32],
        )?;
        telemetry.import_prepare_ms = telemetry
            .import_prepare_ms
            .saturating_add(duration_ms_u64(prepare_started.elapsed()));

        let rows_started = Instant::now();
        let rows = unresolved_edges(conn, EdgeKind::IMPORT, scope_context)?;
        telemetry.import_unresolved_load_ms = telemetry
            .import_unresolved_load_ms
            .saturating_add(duration_ms_u64(rows_started.elapsed()));
        if rows.is_empty() {
            return Ok(0);
        }

        let candidate_started = Instant::now();
        let candidate_index = CandidateIndex::load(
            conn,
            &[
                NodeKind::MODULE as i32,
                NodeKind::NAMESPACE as i32,
                NodeKind::PACKAGE as i32,
            ],
        )?;
        telemetry.import_candidate_index_ms = telemetry
            .import_candidate_index_ms
            .saturating_add(duration_ms_u64(candidate_started.elapsed()));

        let semantic_candidates_by_row =
            self.semantic_candidates_for_rows(conn, &rows, EdgeKind::IMPORT)?;

        let compute_started = Instant::now();
        let computed_results: Vec<Result<ComputedResolution>> =
            if self.flags.parallel_compute && rows.len() > 1 {
                rows.par_iter()
                    .zip(semantic_candidates_by_row.par_iter())
                    .map(|(row, semantic_candidates)| {
                        self.compute_import_resolution(&candidate_index, row, semantic_candidates)
                    })
                    .collect()
            } else {
                rows.iter()
                    .zip(semantic_candidates_by_row.iter())
                    .map(|(row, semantic_candidates)| {
                        self.compute_import_resolution(&candidate_index, row, semantic_candidates)
                    })
                    .collect()
            };
        telemetry.import_compute_ms = telemetry
            .import_compute_ms
            .saturating_add(duration_ms_u64(compute_started.elapsed()));

        let mut resolved = 0usize;
        let mut updates = Vec::with_capacity(rows.len());
        for computed in computed_results {
            let computed = computed?;
            if computed.strategy.is_some() {
                resolved += 1;
            }
            strategy_counters.record(computed.strategy);
            updates.push(computed.update);
        }

        let apply_started = Instant::now();
        apply_resolution_updates(conn, &updates)?;
        telemetry.import_apply_ms = telemetry
            .import_apply_ms
            .saturating_add(duration_ms_u64(apply_started.elapsed()));
        Ok(resolved)
    }

    fn semantic_candidates_for_rows(
        &self,
        conn: &rusqlite::Connection,
        rows: &[UnresolvedEdgeRow],
        edge_kind: EdgeKind,
    ) -> Result<Vec<Vec<SemanticResolutionCandidate>>> {
        if !self.flags.enable_semantic {
            return Ok(vec![Vec::new(); rows.len()]);
        }

        let mut cache: HashMap<SemanticCacheKey, Vec<SemanticResolutionCandidate>> =
            HashMap::new();
        let mut out = Vec::with_capacity(rows.len());
        for (_, file_id, caller_qualified, target_name, caller_file_path, _) in rows {
            out.push(self.semantic_candidates_for_edge(
                conn,
                &mut cache,
                edge_kind,
                *file_id,
                caller_file_path.as_deref(),
                caller_qualified.as_deref(),
                target_name,
            )?);
        }
        Ok(out)
    }

    fn compute_call_resolution(
        &self,
        candidate_index: &CandidateIndex,
        row: &UnresolvedEdgeRow,
        semantic_candidates: &[SemanticResolutionCandidate],
    ) -> Result<ComputedResolution> {
        let (edge_id, file_id, caller_qualified, target_name, _, callsite_identity) = row;
        let prepared_name = PreparedName::new(target_name.clone());
        let is_common_unqualified = is_common_unqualified_call_name(&prepared_name.original);
        let mut selected: Option<(i64, f32, ResolutionStrategy)> = None;
        let mut semantic_fallback: Option<(i64, f32)> = None;
        let mut candidate_ids = OrderedCandidateIds::with_capacity(8);

        for candidate in semantic_candidates {
            candidate_ids.push(candidate.target_node_id);
            consider_selected(
                &mut semantic_fallback,
                candidate.target_node_id,
                candidate.confidence,
            );
        }

        if selected.is_none()
            && !is_common_unqualified
            && let Some(candidate) = candidate_index.find_same_file_readonly(
                *file_id,
                &prepared_name.original,
                &prepared_name.ascii_lower,
            )
        {
            candidate_ids.push(candidate);
            selected = Some((
                candidate,
                self.policy.call_same_file,
                ResolutionStrategy::CallSameFile,
            ));
        }

        if selected.is_none()
            && let Some(prefix) = caller_qualified.as_deref().and_then(module_prefix)
            && let Some(candidate) = candidate_index.find_same_module_readonly(
                &prefix.0,
                prefix.1,
                &prepared_name.original,
                &prepared_name.ascii_lower,
            )
        {
            candidate_ids.push(candidate);
            selected = Some((
                candidate,
                self.policy.call_same_module,
                ResolutionStrategy::CallSameModule,
            ));
        }

        if selected.is_none()
            && !is_common_unqualified
            && let Some(candidate) = candidate_index.find_global_unique_readonly(
                &prepared_name.original,
                &prepared_name.ascii_lower,
            )
        {
            candidate_ids.push(candidate);
            selected = Some((
                candidate,
                self.policy.call_global_unique,
                ResolutionStrategy::CallGlobalUnique,
            ));
        }

        if self.flags.store_candidates && selected.is_none() {
            collect_candidate_pool_from_index(
                candidate_index,
                std::slice::from_ref(&prepared_name),
                &mut candidate_ids,
                6,
            );
        }

        if selected.is_none()
            && let Some((candidate, confidence)) = semantic_fallback
        {
            selected = Some((
                candidate,
                confidence,
                ResolutionStrategy::CallSemanticFallback,
            ));
        }

        if let Some((_, confidence, _)) = selected
            && !should_keep_common_call_resolution(
                &prepared_name.original,
                confidence,
                callsite_identity.as_deref(),
            )
        {
            selected = None;
        }

        let strategy = selected.map(|(_, _, strategy)| strategy);
        let selected_pair = selected.map(|(candidate, confidence, _)| (candidate, confidence));
        let update = build_resolved_edge_update(*edge_id, selected_pair, candidate_ids.as_slice())?;
        Ok(ComputedResolution { update, strategy })
    }

    fn compute_import_resolution(
        &self,
        candidate_index: &CandidateIndex,
        row: &UnresolvedEdgeRow,
        semantic_candidates: &[SemanticResolutionCandidate],
    ) -> Result<ComputedResolution> {
        let (edge_id, file_id, caller_qualified, target_name, _, _) = row;
        let caller_prefix = caller_qualified.as_deref().and_then(module_prefix);
        let name_candidates = import_name_candidates(target_name, self.flags.legacy_mode)
            .into_iter()
            .map(PreparedName::new)
            .collect::<Vec<_>>();

        let mut semantic_fallback: Option<(i64, f32)> = None;
        let mut candidate_ids = OrderedCandidateIds::with_capacity(10);
        for candidate in semantic_candidates {
            candidate_ids.push(candidate.target_node_id);
            consider_selected(
                &mut semantic_fallback,
                candidate.target_node_id,
                candidate.confidence,
            );
        }

        let mut same_file_stage = OrderedCandidateIds::default();
        let mut same_module_stage = OrderedCandidateIds::default();
        let mut global_stage = OrderedCandidateIds::default();
        let mut fuzzy_stage = OrderedCandidateIds::default();

        let mut same_file_selected: Option<i64> = None;
        let mut same_module_selected: Option<i64> = None;
        let mut global_selected: Option<i64> = None;
        let mut fuzzy_selected: Option<i64> = None;

        for name in &name_candidates {
            if self.flags.legacy_mode && same_file_selected.is_none() {
                if let Some(candidate) = candidate_index.find_same_file_readonly(
                    *file_id,
                    &name.original,
                    &name.ascii_lower,
                ) {
                    same_file_stage.push(candidate);
                    same_file_selected = Some(candidate);
                    break;
                }
            }

            if same_module_selected.is_none()
                && let Some(prefix) = caller_prefix.as_ref()
                && let Some(candidate) = candidate_index.find_same_module_readonly(
                    &prefix.0,
                    prefix.1,
                    &name.original,
                    &name.ascii_lower,
                )
            {
                same_module_stage.push(candidate);
                if !candidate_index.is_same_file_candidate(candidate, *file_id) {
                    same_module_selected = Some(candidate);
                }
            }

            if global_selected.is_none()
                && let Some(candidate) = candidate_index
                    .find_global_unique_readonly(&name.original, &name.ascii_lower)
            {
                global_stage.push(candidate);
                if !candidate_index.is_same_file_candidate(candidate, *file_id) {
                    global_selected = Some(candidate);
                }
            }

            if !self.flags.legacy_mode
                && fuzzy_selected.is_none()
                && let Some(candidate) =
                    candidate_index.find_fuzzy_readonly(&name.original, &name.ascii_lower)
            {
                fuzzy_stage.push(candidate);
                if !candidate_index.is_same_file_candidate(candidate, *file_id) {
                    fuzzy_selected = Some(candidate);
                }
            }
        }

        if self.flags.legacy_mode && same_file_selected.is_some() {
            candidate_ids.extend_stage(&same_file_stage.into_vec(), usize::MAX);
        } else {
            candidate_ids.extend_stage(&same_module_stage.into_vec(), usize::MAX);
            if same_module_selected.is_none() {
                candidate_ids.extend_stage(&global_stage.into_vec(), usize::MAX);
                if global_selected.is_none() && !self.flags.legacy_mode {
                    candidate_ids.extend_stage(&fuzzy_stage.into_vec(), usize::MAX);
                }
            }
        }

        if self.flags.store_candidates {
            collect_candidate_pool_from_index(candidate_index, &name_candidates, &mut candidate_ids, 8);
        }

        let mut selected: Option<(i64, f32, ResolutionStrategy)> = if let Some(candidate) =
            same_file_selected
        {
            Some((
                candidate,
                self.policy.import_same_file,
                ResolutionStrategy::ImportSameFile,
            ))
        } else if let Some(candidate) = same_module_selected {
            Some((
                candidate,
                self.policy.import_same_module,
                ResolutionStrategy::ImportSameModule,
            ))
        } else if let Some(candidate) = global_selected {
            Some((
                candidate,
                self.policy.import_global_unique,
                ResolutionStrategy::ImportGlobalUnique,
            ))
        } else if let Some(candidate) = fuzzy_selected {
            Some((candidate, self.policy.import_fuzzy, ResolutionStrategy::ImportFuzzy))
        } else {
            None
        };

        if selected.is_none()
            && let Some((candidate, confidence)) = semantic_fallback
        {
            selected = Some((
                candidate,
                confidence,
                ResolutionStrategy::ImportSemanticFallback,
            ));
        }

        let strategy = selected.map(|(_, _, strategy)| strategy);
        let selected_pair = selected.map(|(candidate, confidence, _)| (candidate, confidence));
        let update = build_resolved_edge_update(*edge_id, selected_pair, candidate_ids.as_slice())?;
        Ok(ComputedResolution { update, strategy })
    }
    #[warn(clippy::too_many_arguments)]
    fn semantic_candidates_for_edge(
        &self,
        conn: &rusqlite::Connection,
        cache: &mut HashMap<SemanticCacheKey, Vec<SemanticResolutionCandidate>>,
        edge_kind: EdgeKind,
        file_id: Option<i64>,
        file_path: Option<&str>,
        caller_qualified: Option<&str>,
        target_name: &str,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let language_bucket = semantic_language_bucket(file_path).map(str::to_string);
        if language_bucket.is_none() {
            return Ok(Vec::new());
        }

        let key = SemanticCacheKey {
            edge_kind,
            file_id,
            caller_qualified: caller_qualified.map(str::to_string),
            target_name: target_name.to_string(),
            language_bucket,
        };

        if let Some(cached) = cache.get(&key) {
            return Ok(cached.clone());
        }

        let request = SemanticResolutionRequest {
            edge_kind,
            file_id,
            file_path: file_path.map(str::to_string),
            caller_qualified: caller_qualified.map(str::to_string),
            target_name: target_name.to_string(),
        };
        let resolved = self.semantic_resolvers.resolve(conn, &request)?;
        cache.insert(key, resolved.clone());
        Ok(resolved)
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

fn apply_resolution_updates(
    conn: &rusqlite::Connection,
    updates: &[ResolvedEdgeUpdate],
) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "UPDATE edge
         SET resolved_target_node_id = ?1,
             confidence = ?2,
             certainty = ?3,
             candidate_target_node_ids = ?4
         WHERE id = ?5",
    )?;
    for update in updates {
        stmt.execute(params![
            update.resolved_target_node_id,
            update.confidence,
            update.certainty,
            update.candidate_payload.as_deref(),
            update.edge_id
        ])?;
    }
    Ok(())
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
    let file_path = file_path?;
    let ext = file_path.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "ts" | "tsx" => Some("typescript"),
        "java" => Some("java"),
        _ => None,
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

        let mut index = CandidateIndex::default();
        for row in rows {
            index.nodes.push(row?);
        }

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
        Ok(index)
    }

    #[cfg(test)]
    fn find_same_file(&mut self, file_id: Option<i64>, name: &str) -> Option<i64> {
        let name_ascii_lower = name.to_ascii_lowercase();
        self.find_same_file_prepared(file_id, name, &name_ascii_lower)
    }

    #[cfg(test)]
    fn find_same_file_prepared(
        &mut self,
        file_id: Option<i64>,
        name: &str,
        name_ascii_lower: &str,
    ) -> Option<i64> {
        let file_id = file_id?;
        let key = (file_id, name.to_string());
        if let Some(cached) = self.same_file_cache.get(&key) {
            return *cached;
        }
        let resolved = self.find_same_file_readonly(Some(file_id), name, name_ascii_lower);
        self.same_file_cache.insert(key, resolved);
        resolved
    }

    fn find_same_file_readonly(
        &self,
        file_id: Option<i64>,
        name: &str,
        name_ascii_lower: &str,
    ) -> Option<i64> {
        let file_id = file_id?;
        self.first_in_file(self.exact_map.get(name), file_id)
            .or_else(|| self.first_in_file(self.suffix_map_ascii_lower.get(name_ascii_lower), file_id))
    }

    #[cfg(test)]
    fn find_same_module(
        &mut self,
        module_prefix: &str,
        delimiter: &str,
        name: &str,
    ) -> Option<i64> {
        let name_ascii_lower = name.to_ascii_lowercase();
        self.find_same_module_prepared(module_prefix, delimiter, name, &name_ascii_lower)
    }

    #[cfg(test)]
    fn find_same_module_prepared(
        &mut self,
        module_prefix: &str,
        delimiter: &str,
        name: &str,
        name_ascii_lower: &str,
    ) -> Option<i64> {
        let qualified_prefix = format!("{module_prefix}{delimiter}");
        let key = (qualified_prefix.clone(), name.to_string());
        if let Some(cached) = self.same_module_cache.get(&key) {
            return *cached;
        }
        let resolved = self.find_same_module_readonly(
            module_prefix,
            delimiter,
            name,
            name_ascii_lower,
        );
        self.same_module_cache.insert(key, resolved);
        resolved
    }

    fn find_same_module_readonly(
        &self,
        module_prefix: &str,
        delimiter: &str,
        name: &str,
        name_ascii_lower: &str,
    ) -> Option<i64> {
        let qualified_prefix = format!("{module_prefix}{delimiter}");
        self.first_in_module(self.exact_map.get(name), &qualified_prefix)
            .or_else(|| {
                self.first_in_module(
                    self.suffix_map_ascii_lower.get(name_ascii_lower),
                    &qualified_prefix,
                )
            })
    }

    #[cfg(test)]
    fn find_global_unique(&mut self, name: &str) -> Option<i64> {
        let name_ascii_lower = name.to_ascii_lowercase();
        self.find_global_unique_prepared(name, &name_ascii_lower)
    }

    #[cfg(test)]
    fn find_global_unique_prepared(&mut self, name: &str, name_ascii_lower: &str) -> Option<i64> {
        if let Some(cached) = self.global_unique_cache.get(name) {
            return *cached;
        }
        let resolved = self.find_global_unique_readonly(name, name_ascii_lower);
        self.global_unique_cache.insert(name.to_string(), resolved);
        resolved
    }

    fn find_global_unique_readonly(&self, name: &str, name_ascii_lower: &str) -> Option<i64> {
        if let Some(exact) = self.exact_map.get(name) {
            if exact.len() == 1 {
                return Some(self.nodes[exact[0]].id);
            }
            return None;
        }
        if let Some(suffix) = self.suffix_map_ascii_lower.get(name_ascii_lower) {
            if suffix.len() == 1 {
                return Some(self.nodes[suffix[0]].id);
            }
        }
        None
    }

    #[cfg(test)]
    fn find_fuzzy(&mut self, name: &str) -> Option<i64> {
        let name_ascii_lower = name.to_ascii_lowercase();
        if let Some(cached) = self.fuzzy_cache_ascii_lower.get(&name_ascii_lower) {
            return *cached;
        }
        let resolved = self.find_fuzzy_readonly(name, &name_ascii_lower);
        self.fuzzy_cache_ascii_lower
            .insert(name_ascii_lower, resolved);
        resolved
    }

    fn find_fuzzy_readonly(&self, name: &str, name_ascii_lower: &str) -> Option<i64> {
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
        if out.len() < limit {
            if let Some(suffix) = self.suffix_map_ascii_lower.get(name_ascii_lower) {
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

fn cleanup_stale_call_resolutions(
    conn: &rusqlite::Connection,
    flags: ResolutionFlags,
    policy: ResolutionPolicy,
    scope_context: &ScopeCallerContext,
) -> Result<()> {
    if scope_context.is_empty() {
        return Ok(());
    }

    let cutoff = policy.min_call_confidence;
    let mut low_confidence_query = String::from(
        "UPDATE edge SET resolved_target_node_id = NULL, confidence = NULL, certainty = NULL
         WHERE kind = ?1 AND confidence IS NOT NULL AND confidence < ?2",
    );
    if scope_context.is_scoped() {
        low_confidence_query.push_str(&format!(
            " AND source_node_id IN (SELECT caller_id FROM {SCOPED_CALLER_TABLE})"
        ));
    }
    conn.execute(
        &low_confidence_query,
        params![EdgeKind::CALL as i64, cutoff as f64],
    )?;

    let common_names = common_unqualified_call_names();
    if common_names.is_empty() {
        return Ok(());
    }
    let names_placeholders = question_placeholders(common_names.len());

    if flags.legacy_mode {
        let mut legacy_query = format!(
            "UPDATE edge SET resolved_target_node_id = NULL, confidence = NULL, certainty = NULL
             WHERE kind = ?
             AND resolved_target_node_id IS NOT NULL
             AND target_node_id IN (SELECT id FROM node WHERE serialized_name IN ({}))",
            names_placeholders
        );
        if scope_context.is_scoped() {
            legacy_query.push_str(&format!(
                " AND source_node_id IN (SELECT caller_id FROM {SCOPED_CALLER_TABLE})"
            ));
        }
        let mut legacy_params = vec![Value::Integer(EdgeKind::CALL as i64)];
        legacy_params.extend(
            common_names
                .iter()
                .map(|name| Value::Text((*name).to_string())),
        );
        conn.execute(&legacy_query, params_from_iter(legacy_params.iter()))?;
    } else {
        let mut strict_query = format!(
            "UPDATE edge SET resolved_target_node_id = NULL, confidence = NULL, certainty = NULL
             WHERE kind = ?
             AND resolved_target_node_id IS NOT NULL
             AND target_node_id IN (SELECT id FROM node WHERE serialized_name IN ({}))
             AND (certainty IS NULL OR certainty != ?)",
            names_placeholders
        );
        if scope_context.is_scoped() {
            strict_query.push_str(&format!(
                " AND source_node_id IN (SELECT caller_id FROM {SCOPED_CALLER_TABLE})"
            ));
        }
        let mut strict_params = vec![Value::Integer(EdgeKind::CALL as i64)];
        strict_params.extend(
            common_names
                .iter()
                .map(|name| Value::Text((*name).to_string())),
        );
        strict_params.push(Value::Text(
            ResolutionCertainty::Certain.as_str().to_string(),
        ));
        conn.execute(&strict_query, params_from_iter(strict_params.iter()))?;
    }

    Ok(())
}

fn unresolved_edges(
    conn: &rusqlite::Connection,
    kind: EdgeKind,
    scope_context: &ScopeCallerContext,
) -> Result<Vec<UnresolvedEdgeRow>> {
    if scope_context.is_empty() {
        return Ok(Vec::new());
    }

    let mut query = String::from(
        "SELECT e.id, caller.file_node_id, caller.qualified_name, target.serialized_name, file_node.serialized_name, e.callsite_identity
         FROM edge e
         JOIN node caller ON caller.id = e.source_node_id
         JOIN node target ON target.id = e.target_node_id
         LEFT JOIN node file_node ON file_node.id = caller.file_node_id
         WHERE e.kind = ?1 AND e.resolved_target_node_id IS NULL",
    );
    if scope_context.is_scoped() {
        query.push_str(&format!(
            " AND e.source_node_id IN (SELECT caller_id FROM {SCOPED_CALLER_TABLE})"
        ));
    }
    query.push_str(" ORDER BY e.id");
    let mut stmt = conn.prepare(&query)?;

    let rows = stmt.query_map(params![kind as i32], map_unresolved_edge_row)?;
    let collected = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(collected)
}

fn map_unresolved_edge_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UnresolvedEdgeRow> {
    Ok((
        row.get::<_, i64>(0)?,
        row.get::<_, Option<i64>>(1)?,
        row.get::<_, Option<String>>(2)?,
        row.get::<_, String>(3)?,
        row.get::<_, Option<String>>(4)?,
        row.get::<_, Option<String>>(5)?,
    ))
}

fn sorted_scope_file_ids(caller_scope_file_ids: Option<&HashSet<i64>>) -> Option<Vec<i64>> {
    caller_scope_file_ids.map(|scope| {
        let mut ids = scope.iter().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    })
}

fn numbered_placeholders(start: usize, count: usize) -> String {
    (0..count)
        .map(|offset| format!("?{}", start + offset))
        .collect::<Vec<_>>()
        .join(", ")
}

fn question_placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
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
        "collect",
        "dedup",
        "extend",
        "is_empty",
        "insert",
        "len",
        "map",
        "map_err",
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
    common_unqualified_call_names().contains(&name)
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
    fn test_common_unqualified_call_names_include_clone_like_noise() {
        assert!(is_common_unqualified_call_name("clone"));
        assert!(is_common_unqualified_call_name("len"));
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
        let scoped_rows = unresolved_edges(&conn, EdgeKind::CALL, &scoped_context)?;
        assert_eq!(scoped_rows.len(), 1);
        assert_eq!(scoped_rows[0].0, 1000_i64);
        assert_eq!(scoped_rows[0].1, Some(100_i64));

        let all_rows = unresolved_edges(&conn, EdgeKind::CALL, &unscoped_context)?;
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

        let mut index = CandidateIndex::load(&conn, &[NodeKind::FUNCTION as i32])?;
        assert_eq!(index.find_same_file(Some(101_i64), "target"), Some(10_i64));
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

        let mut index = CandidateIndex::load(&conn, &[NodeKind::FUNCTION as i32])?;
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

        let mut index = CandidateIndex::load(&conn, &[NodeKind::MODULE as i32])?;
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
        cleanup_stale_call_resolutions(&conn, flags, policy, &scope_context)?;

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
