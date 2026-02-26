use crate::semantic::{SemanticResolutionRequest, SemanticResolverRegistry};
use anyhow::Result;
use codestory_core::{EdgeKind, NodeKind, ResolutionCertainty};
use codestory_storage::Storage;
use rusqlite::{OptionalExtension, params, params_from_iter, types::Value};
use std::collections::{HashMap, HashSet};

type UnresolvedEdgeRow = (
    i64,
    Option<i64>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);

#[derive(Default)]
struct ResolutionLookupCache {
    same_file_lookup: HashMap<(String, i64, String), Option<i64>>,
    same_module_lookup: HashMap<(String, String, String), Option<i64>>,
    global_unique_lookup: HashMap<(String, String), Option<i64>>,
}

#[derive(Default, Debug)]
pub struct ResolutionStats {
    pub unresolved_calls_before: usize,
    pub resolved_calls: usize,
    pub unresolved_calls: usize,
    pub unresolved_imports_before: usize,
    pub resolved_imports: usize,
    pub unresolved_imports: usize,
}

#[derive(Debug, Clone, Copy)]
struct ResolutionFlags {
    legacy_mode: bool,
    enable_semantic: bool,
    store_candidates: bool,
}

impl ResolutionFlags {
    fn from_env() -> Self {
        let legacy_mode = env_flag("CODESTORY_RESOLUTION_LEGACY_MODE", false)
            || env_flag("CODESTORY_RESOLUTION_LEGACY", false);
        Self {
            legacy_mode,
            enable_semantic: env_flag("CODESTORY_RESOLUTION_ENABLE_SEMANTIC", !legacy_mode),
            store_candidates: env_flag("CODESTORY_RESOLUTION_STORE_CANDIDATES", !legacy_mode),
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
            let unresolved_calls_before =
                Self::count_unresolved_on_conn(conn, EdgeKind::CALL, caller_scope_file_ids)?;
            let unresolved_imports_before =
                Self::count_unresolved_on_conn(conn, EdgeKind::IMPORT, caller_scope_file_ids)?;
            let resolved_calls = self.resolve_calls_on_conn(conn, caller_scope_file_ids)?;
            let resolved_imports = self.resolve_imports_on_conn(conn, caller_scope_file_ids)?;
            let unresolved_calls =
                Self::count_unresolved_on_conn(conn, EdgeKind::CALL, caller_scope_file_ids)?;
            let unresolved_imports =
                Self::count_unresolved_on_conn(conn, EdgeKind::IMPORT, caller_scope_file_ids)?;

            Ok(ResolutionStats {
                unresolved_calls_before,
                resolved_calls,
                unresolved_calls,
                unresolved_imports_before,
                resolved_imports,
                unresolved_imports,
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
        Ok((
            self.count_unresolved(storage, EdgeKind::CALL, caller_scope_file_ids)?,
            self.count_unresolved(storage, EdgeKind::IMPORT, caller_scope_file_ids)?,
        ))
    }

    fn count_unresolved(
        &self,
        storage: &Storage,
        kind: EdgeKind,
        caller_scope_file_ids: Option<&HashSet<i64>>,
    ) -> Result<usize> {
        Self::count_unresolved_on_conn(storage.get_connection(), kind, caller_scope_file_ids)
    }

    fn count_unresolved_on_conn(
        conn: &rusqlite::Connection,
        kind: EdgeKind,
        caller_scope_file_ids: Option<&HashSet<i64>>,
    ) -> Result<usize> {
        let scope_file_ids = sorted_scope_file_ids(caller_scope_file_ids);
        if matches!(scope_file_ids.as_ref(), Some(ids) if ids.is_empty()) {
            return Ok(0);
        }

        let mut query = String::from("SELECT COUNT(*) FROM edge e");
        if scope_file_ids.is_some() {
            query.push_str(" JOIN node caller ON caller.id = e.source_node_id");
        }
        query.push_str(" WHERE e.kind = ?1 AND e.resolved_target_node_id IS NULL");

        let count: i64 = if let Some(scope_ids) = scope_file_ids.as_ref() {
            query.push_str(" AND caller.file_node_id IN (");
            query.push_str(&numbered_placeholders(2, scope_ids.len()));
            query.push(')');
            let params = kind_scope_params(kind, scope_ids);
            conn.query_row(&query, params_from_iter(params.iter()), |row| row.get(0))?
        } else {
            conn.query_row(&query, params![kind as i32], |row| row.get(0))?
        };
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
        self.resolve_calls_on_conn(storage.get_connection(), caller_scope_file_ids)
    }

    fn resolve_calls_on_conn(
        &self,
        conn: &rusqlite::Connection,
        caller_scope_file_ids: Option<&HashSet<i64>>,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE edge SET resolved_source_node_id = source_node_id
             WHERE kind = ?1 AND resolved_source_node_id IS NULL",
            params![EdgeKind::CALL as i32],
        )?;

        cleanup_stale_call_resolutions(conn, self.flags, self.policy, caller_scope_file_ids)?;

        let mut resolved = 0usize;
        let rows = unresolved_edges(conn, EdgeKind::CALL, caller_scope_file_ids)?;
        let function_kinds = [
            NodeKind::FUNCTION as i32,
            NodeKind::METHOD as i32,
            NodeKind::MACRO as i32,
        ];
        let function_kind_clause = kind_clause(&function_kinds);
        let mut lookup_cache = ResolutionLookupCache::default();

        for (
            edge_id,
            file_id,
            caller_qualified,
            target_name,
            caller_file_path,
            callsite_identity,
        ) in rows
        {
            let mut selected: Option<(i64, f32)> = None;
            let mut semantic_fallback: Option<(i64, f32)> = None;
            let mut candidate_ids: Vec<i64> = Vec::new();
            let is_common_unqualified = is_common_unqualified_call_name(&target_name);
            let (exact, suffix_dot, suffix_colon) = name_patterns(&target_name);

            if self.flags.enable_semantic {
                let request = SemanticResolutionRequest {
                    edge_kind: EdgeKind::CALL,
                    file_id,
                    file_path: caller_file_path.clone(),
                    caller_qualified: caller_qualified.clone(),
                    target_name: target_name.clone(),
                };
                let semantic_candidates = self.semantic_resolvers.resolve(conn, &request)?;
                for candidate in semantic_candidates {
                    record_candidate(&mut candidate_ids, candidate.target_node_id);
                    consider_selected(
                        &mut semantic_fallback,
                        candidate.target_node_id,
                        candidate.confidence,
                    );
                }
            }

            if selected.is_none()
                && !is_common_unqualified
                && let Some(candidate) = find_same_file(
                    conn,
                    &function_kind_clause,
                    file_id,
                    &exact,
                    &suffix_dot,
                    &suffix_colon,
                    &mut lookup_cache,
                )?
            {
                record_candidate(&mut candidate_ids, candidate);
                selected = Some((candidate, self.policy.call_same_file));
            }

            if selected.is_none()
                && let Some(prefix) = caller_qualified.and_then(module_prefix)
                && let Some(candidate) = find_same_module(
                    conn,
                    &function_kind_clause,
                    &prefix.0,
                    prefix.1,
                    &exact,
                    &suffix_dot,
                    &suffix_colon,
                    &mut lookup_cache,
                )?
            {
                record_candidate(&mut candidate_ids, candidate);
                selected = Some((candidate, self.policy.call_same_module));
            }

            if selected.is_none()
                && !is_common_unqualified
                && let Some(candidate) = find_global_unique(
                    conn,
                    &function_kind_clause,
                    &exact,
                    &suffix_dot,
                    &suffix_colon,
                    &mut lookup_cache,
                )?
            {
                record_candidate(&mut candidate_ids, candidate);
                selected = Some((candidate, self.policy.call_global_unique));
            }

            if self.flags.store_candidates && selected.is_none() {
                let names = vec![target_name.clone()];
                collect_candidate_pool(conn, &function_kind_clause, &names, &mut candidate_ids, 6)?;
            }

            if selected.is_none() {
                selected = semantic_fallback;
            }

            if let Some((_, confidence)) = selected
                && !should_keep_common_call_resolution(
                    &target_name,
                    confidence,
                    callsite_identity.as_deref(),
                )
            {
                selected = None;
            }

            persist_resolution(conn, edge_id, selected, &candidate_ids)?;
            if selected.is_some() {
                resolved += 1;
            }
        }

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
        self.resolve_imports_on_conn(storage.get_connection(), caller_scope_file_ids)
    }

    fn resolve_imports_on_conn(
        &self,
        conn: &rusqlite::Connection,
        caller_scope_file_ids: Option<&HashSet<i64>>,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE edge SET resolved_source_node_id = source_node_id
             WHERE kind = ?1 AND resolved_source_node_id IS NULL",
            params![EdgeKind::IMPORT as i32],
        )?;

        let mut resolved = 0usize;
        let rows = unresolved_edges(conn, EdgeKind::IMPORT, caller_scope_file_ids)?;
        let mut lookup_cache = ResolutionLookupCache::default();

        let module_kinds = [
            NodeKind::MODULE as i32,
            NodeKind::NAMESPACE as i32,
            NodeKind::PACKAGE as i32,
        ];
        let module_kind_clause = kind_clause(&module_kinds);

        for (edge_id, file_id, caller_qualified, target_name, caller_file_path, _) in rows {
            let mut selected: Option<(i64, f32)> = None;
            let mut semantic_fallback: Option<(i64, f32)> = None;
            let mut candidate_ids: Vec<i64> = Vec::new();
            let names = import_name_candidates(&target_name, self.flags.legacy_mode);

            if self.flags.enable_semantic {
                let request = SemanticResolutionRequest {
                    edge_kind: EdgeKind::IMPORT,
                    file_id,
                    file_path: caller_file_path.clone(),
                    caller_qualified: caller_qualified.clone(),
                    target_name: target_name.clone(),
                };
                let semantic_candidates = self.semantic_resolvers.resolve(conn, &request)?;
                for candidate in semantic_candidates {
                    record_candidate(&mut candidate_ids, candidate.target_node_id);
                    consider_selected(
                        &mut semantic_fallback,
                        candidate.target_node_id,
                        candidate.confidence,
                    );
                }
            }

            if self.flags.legacy_mode {
                for name in &names {
                    if selected.is_some() {
                        break;
                    }
                    let (exact, suffix_dot, suffix_colon) = name_patterns(name);
                    if let Some(candidate) = find_same_file(
                        conn,
                        &module_kind_clause,
                        file_id,
                        &exact,
                        &suffix_dot,
                        &suffix_colon,
                        &mut lookup_cache,
                    )? {
                        record_candidate(&mut candidate_ids, candidate);
                        selected = Some((candidate, self.policy.import_same_file));
                    }
                }
            }

            for name in &names {
                if selected.is_some() {
                    break;
                }
                if let Some(prefix) = caller_qualified.clone().and_then(module_prefix) {
                    let (exact, suffix_dot, suffix_colon) = name_patterns(name);
                    if let Some(candidate) = find_same_module(
                        conn,
                        &module_kind_clause,
                        &prefix.0,
                        prefix.1,
                        &exact,
                        &suffix_dot,
                        &suffix_colon,
                        &mut lookup_cache,
                    )? {
                        record_candidate(&mut candidate_ids, candidate);
                        if !is_same_file_candidate(conn, candidate, file_id)? {
                            selected = Some((candidate, self.policy.import_same_module));
                        }
                    }
                }
            }

            for name in &names {
                if selected.is_some() {
                    break;
                }
                let (exact, suffix_dot, suffix_colon) = name_patterns(name);
                if let Some(candidate) = find_global_unique(
                    conn,
                    &module_kind_clause,
                    &exact,
                    &suffix_dot,
                    &suffix_colon,
                    &mut lookup_cache,
                )? {
                    record_candidate(&mut candidate_ids, candidate);
                    if !is_same_file_candidate(conn, candidate, file_id)? {
                        selected = Some((candidate, self.policy.import_global_unique));
                    }
                }
            }

            if selected.is_none() && !self.flags.legacy_mode {
                for name in &names {
                    let (exact, suffix_dot, suffix_colon) = name_patterns(name);
                    if let Some(candidate) = find_fuzzy(
                        conn,
                        &module_kind_clause,
                        &exact,
                        &suffix_dot,
                        &suffix_colon,
                    )? {
                        record_candidate(&mut candidate_ids, candidate);
                        if !is_same_file_candidate(conn, candidate, file_id)? {
                            selected = Some((candidate, self.policy.import_fuzzy));
                            break;
                        }
                    }
                }
            }

            if self.flags.store_candidates {
                collect_candidate_pool(conn, &module_kind_clause, &names, &mut candidate_ids, 8)?;
            }

            if selected.is_none() {
                selected = semantic_fallback;
            }

            persist_resolution(conn, edge_id, selected, &candidate_ids)?;
            if selected.is_some() {
                resolved += 1;
            }
        }

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

fn persist_resolution(
    conn: &rusqlite::Connection,
    edge_id: i64,
    selected: Option<(i64, f32)>,
    candidates: &[i64],
) -> Result<usize> {
    let candidate_payload = candidate_json(candidates)?;
    if let Some((resolved_target, confidence)) = selected {
        let certainty =
            ResolutionCertainty::from_confidence(Some(confidence)).map(ResolutionCertainty::as_str);
        return Ok(conn.execute(
            "UPDATE edge
             SET resolved_target_node_id = ?1,
                 confidence = ?2,
                 certainty = ?3,
                 candidate_target_node_ids = ?4
             WHERE id = ?5",
            params![
                resolved_target,
                confidence,
                certainty,
                candidate_payload,
                edge_id
            ],
        )?);
    }

    Ok(conn.execute(
        "UPDATE edge
         SET resolved_target_node_id = NULL,
             confidence = NULL,
             certainty = NULL,
             candidate_target_node_ids = ?1
         WHERE id = ?2",
        params![candidate_payload, edge_id],
    )?)
}

fn cleanup_stale_call_resolutions(
    conn: &rusqlite::Connection,
    flags: ResolutionFlags,
    policy: ResolutionPolicy,
    caller_scope_file_ids: Option<&HashSet<i64>>,
) -> Result<()> {
    let cutoff = policy.min_call_confidence;
    let scope_file_ids = sorted_scope_file_ids(caller_scope_file_ids);
    let mut low_confidence_query = String::from(
        "UPDATE edge SET resolved_target_node_id = NULL, confidence = NULL, certainty = NULL
         WHERE kind = ?1 AND confidence IS NOT NULL AND confidence < ?2",
    );
    if let Some(scope_ids) = scope_file_ids.as_ref() {
        low_confidence_query
            .push_str(" AND source_node_id IN (SELECT id FROM node WHERE file_node_id IN (");
        low_confidence_query.push_str(&numbered_placeholders(3, scope_ids.len()));
        low_confidence_query.push_str("))");
    }
    let mut low_confidence_params = vec![
        Value::Integer(EdgeKind::CALL as i64),
        Value::Real(cutoff as f64),
    ];
    if let Some(scope_ids) = scope_file_ids.as_ref() {
        low_confidence_params.extend(scope_ids.iter().copied().map(Value::Integer));
    }
    conn.execute(
        &low_confidence_query,
        params_from_iter(low_confidence_params.iter()),
    )?;

    for common_name in common_unqualified_call_names() {
        if flags.legacy_mode {
            let mut legacy_query = String::from(
                "UPDATE edge SET resolved_target_node_id = NULL, confidence = NULL, certainty = NULL
                 WHERE kind = ?1
                 AND resolved_target_node_id IS NOT NULL
                 AND target_node_id IN (SELECT id FROM node WHERE serialized_name = ?2)",
            );
            if let Some(scope_ids) = scope_file_ids.as_ref() {
                legacy_query.push_str(
                    " AND source_node_id IN (SELECT id FROM node WHERE file_node_id IN (",
                );
                legacy_query.push_str(&numbered_placeholders(3, scope_ids.len()));
                legacy_query.push_str("))");
            }
            let mut legacy_params = vec![
                Value::Integer(EdgeKind::CALL as i64),
                Value::Text((*common_name).to_string()),
            ];
            if let Some(scope_ids) = scope_file_ids.as_ref() {
                legacy_params.extend(scope_ids.iter().copied().map(Value::Integer));
            }
            conn.execute(&legacy_query, params_from_iter(legacy_params.iter()))?;
        } else {
            let mut strict_query = String::from(
                "UPDATE edge SET resolved_target_node_id = NULL, confidence = NULL, certainty = NULL
                 WHERE kind = ?1
                 AND resolved_target_node_id IS NOT NULL
                 AND target_node_id IN (SELECT id FROM node WHERE serialized_name = ?2)
                 AND (certainty IS NULL OR certainty != ?3)",
            );
            if let Some(scope_ids) = scope_file_ids.as_ref() {
                strict_query.push_str(
                    " AND source_node_id IN (SELECT id FROM node WHERE file_node_id IN (",
                );
                strict_query.push_str(&numbered_placeholders(4, scope_ids.len()));
                strict_query.push_str("))");
            }
            let mut strict_params = vec![
                Value::Integer(EdgeKind::CALL as i64),
                Value::Text((*common_name).to_string()),
                Value::Text(ResolutionCertainty::Certain.as_str().to_string()),
            ];
            if let Some(scope_ids) = scope_file_ids.as_ref() {
                strict_params.extend(scope_ids.iter().copied().map(Value::Integer));
            }
            conn.execute(&strict_query, params_from_iter(strict_params.iter()))?;
        }
    }

    Ok(())
}

fn unresolved_edges(
    conn: &rusqlite::Connection,
    kind: EdgeKind,
    caller_scope_file_ids: Option<&HashSet<i64>>,
) -> Result<Vec<UnresolvedEdgeRow>> {
    let scope_file_ids = sorted_scope_file_ids(caller_scope_file_ids);
    if matches!(scope_file_ids.as_ref(), Some(ids) if ids.is_empty()) {
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
    if let Some(scope_ids) = scope_file_ids.as_ref() {
        query.push_str(" AND caller.file_node_id IN (");
        query.push_str(&numbered_placeholders(2, scope_ids.len()));
        query.push(')');
    }
    let mut stmt = conn.prepare(&query)?;

    let collected = if let Some(scope_ids) = scope_file_ids.as_ref() {
        let params = kind_scope_params(kind, scope_ids);
        let rows = stmt.query_map(params_from_iter(params.iter()), map_unresolved_edge_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        let rows = stmt.query_map(params![kind as i32], map_unresolved_edge_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
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

fn kind_scope_params(kind: EdgeKind, scope_file_ids: &[i64]) -> Vec<Value> {
    let mut values = Vec::with_capacity(1 + scope_file_ids.len());
    values.push(Value::Integer(kind as i64));
    values.extend(scope_file_ids.iter().copied().map(Value::Integer));
    values
}

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

fn module_prefix(qualified: String) -> Option<(String, &'static str)> {
    if let Some(idx) = qualified.rfind("::") {
        return Some((qualified[..idx].to_string(), "::"));
    }
    if let Some(idx) = qualified.rfind('.') {
        return Some((qualified[..idx].to_string(), "."));
    }
    None
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, params};
    use std::collections::HashSet;
    use std::time::Instant;
    use tempfile::tempdir;

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
        let scoped_rows = unresolved_edges(&conn, EdgeKind::CALL, Some(&scope))?;
        assert_eq!(scoped_rows.len(), 1);
        assert_eq!(scoped_rows[0].0, 1000_i64);
        assert_eq!(scoped_rows[0].1, Some(100_i64));

        let all_rows = unresolved_edges(&conn, EdgeKind::CALL, None)?;
        assert_eq!(all_rows.len(), 2);

        let scoped_count =
            ResolutionPass::count_unresolved_on_conn(&conn, EdgeKind::CALL, Some(&scope))?;
        let all_count = ResolutionPass::count_unresolved_on_conn(&conn, EdgeKind::CALL, None)?;
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
        };
        let policy = ResolutionPolicy::for_flags(flags);
        let scope = HashSet::from([100_i64]);
        cleanup_stale_call_resolutions(&conn, flags, policy, Some(&scope))?;

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
