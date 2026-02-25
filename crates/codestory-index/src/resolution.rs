use crate::semantic::{SemanticResolutionRequest, SemanticResolverRegistry};
use anyhow::Result;
use codestory_core::{EdgeKind, NodeKind, ResolutionCertainty};
use codestory_storage::Storage;
use rusqlite::{OptionalExtension, params};

type UnresolvedEdgeRow = (i64, Option<i64>, Option<String>, String, Option<String>);

#[derive(Default, Debug)]
pub struct ResolutionStats {
    pub resolved_calls: usize,
    pub unresolved_calls: usize,
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
        let resolved_calls = self.resolve_calls(storage)?;
        let resolved_imports = self.resolve_imports(storage)?;
        let unresolved_calls = self.count_unresolved(storage, EdgeKind::CALL)?;
        let unresolved_imports = self.count_unresolved(storage, EdgeKind::IMPORT)?;

        Ok(ResolutionStats {
            resolved_calls,
            unresolved_calls,
            resolved_imports,
            unresolved_imports,
        })
    }

    fn count_unresolved(&self, storage: &Storage, kind: EdgeKind) -> Result<usize> {
        let conn = storage.get_connection();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM edge WHERE kind = ?1 AND resolved_target_node_id IS NULL",
            params![kind as i32],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    pub fn resolve_calls(&self, storage: &mut Storage) -> Result<usize> {
        let conn = storage.get_connection();
        conn.execute(
            "UPDATE edge SET resolved_source_node_id = source_node_id
             WHERE kind = ?1 AND resolved_source_node_id IS NULL",
            params![EdgeKind::CALL as i32],
        )?;

        cleanup_stale_call_resolutions(conn, self.flags, self.policy)?;

        let mut resolved = 0usize;
        let rows = unresolved_edges(conn, EdgeKind::CALL)?;
        let function_kinds = [
            NodeKind::FUNCTION as i32,
            NodeKind::METHOD as i32,
            NodeKind::MACRO as i32,
        ];
        let function_kind_clause = kind_clause(&function_kinds);

        for (edge_id, file_id, caller_qualified, target_name, caller_file_path) in rows {
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

            persist_resolution(conn, edge_id, selected, &candidate_ids)?;
            if selected.is_some() {
                resolved += 1;
            }
        }

        Ok(resolved)
    }

    pub fn resolve_imports(&self, storage: &mut Storage) -> Result<usize> {
        let conn = storage.get_connection();
        conn.execute(
            "UPDATE edge SET resolved_source_node_id = source_node_id
             WHERE kind = ?1 AND resolved_source_node_id IS NULL",
            params![EdgeKind::IMPORT as i32],
        )?;

        let mut resolved = 0usize;
        let rows = unresolved_edges(conn, EdgeKind::IMPORT)?;

        let module_kinds = [
            NodeKind::MODULE as i32,
            NodeKind::NAMESPACE as i32,
            NodeKind::PACKAGE as i32,
        ];
        let module_kind_clause = kind_clause(&module_kinds);

        for (edge_id, file_id, caller_qualified, target_name, caller_file_path) in rows {
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
) -> Result<()> {
    let cutoff = policy.min_call_confidence;
    conn.execute(
        "UPDATE edge SET resolved_target_node_id = NULL, confidence = NULL, certainty = NULL
         WHERE kind = ?1 AND confidence IS NOT NULL AND confidence < ?2",
        params![EdgeKind::CALL as i32, cutoff],
    )?;

    for common_name in common_unqualified_call_names() {
        if flags.legacy_mode {
            conn.execute(
                "UPDATE edge SET resolved_target_node_id = NULL, confidence = NULL, certainty = NULL
                 WHERE kind = ?1
                 AND resolved_target_node_id IS NOT NULL
                 AND target_node_id IN (SELECT id FROM node WHERE serialized_name = ?2)",
                params![EdgeKind::CALL as i32, common_name],
            )?;
        } else {
            conn.execute(
                "UPDATE edge SET resolved_target_node_id = NULL, confidence = NULL, certainty = NULL
                 WHERE kind = ?1
                 AND resolved_target_node_id IS NOT NULL
                 AND target_node_id IN (SELECT id FROM node WHERE serialized_name = ?2)
                 AND (certainty IS NULL OR certainty != ?3)",
                params![
                    EdgeKind::CALL as i32,
                    common_name,
                    ResolutionCertainty::Certain.as_str()
                ],
            )?;
        }
    }

    Ok(())
}

fn unresolved_edges(conn: &rusqlite::Connection, kind: EdgeKind) -> Result<Vec<UnresolvedEdgeRow>> {
    let mut stmt = conn.prepare(
        "SELECT e.id, caller.file_node_id, caller.qualified_name, target.serialized_name, file_node.serialized_name
         FROM edge e
         JOIN node caller ON caller.id = e.source_node_id
         JOIN node target ON target.id = e.target_node_id
         LEFT JOIN node file_node ON file_node.id = caller.file_node_id
         WHERE e.kind = ?1 AND e.resolved_target_node_id IS NULL",
    )?;

    let rows = stmt.query_map(params![kind as i32], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;

    let collected = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(collected)
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
) -> Result<Option<i64>> {
    let Some(file_id) = file_id else {
        return Ok(None);
    };
    let query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND file_node_id = ?1
         AND (serialized_name = ?2 OR serialized_name LIKE ?3 OR serialized_name LIKE ?4)
         ORDER BY start_line LIMIT 1",
        kind_clause
    );
    conn.query_row(
        &query,
        params![file_id, exact, suffix_dot, suffix_colon],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn find_same_module(
    conn: &rusqlite::Connection,
    kind_clause: &str,
    module_prefix: &str,
    delimiter: &str,
    exact: &str,
    suffix_dot: &str,
    suffix_colon: &str,
) -> Result<Option<i64>> {
    let pattern = format!("{}{}%", module_prefix, delimiter);
    let query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND qualified_name LIKE ?1
         AND (serialized_name = ?2 OR serialized_name LIKE ?3 OR serialized_name LIKE ?4)
         ORDER BY start_line LIMIT 1",
        kind_clause
    );
    conn.query_row(
        &query,
        params![pattern, exact, suffix_dot, suffix_colon],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn find_global_unique(
    conn: &rusqlite::Connection,
    kind_clause: &str,
    exact: &str,
    suffix_dot: &str,
    suffix_colon: &str,
) -> Result<Option<i64>> {
    let count_query = format!(
        "SELECT COUNT(*) FROM node
         WHERE kind IN ({})
         AND (serialized_name = ?1 OR serialized_name LIKE ?2 OR serialized_name LIKE ?3)",
        kind_clause
    );
    let count: i64 = conn.query_row(
        &count_query,
        params![exact, suffix_dot, suffix_colon],
        |row| row.get(0),
    )?;
    if count != 1 {
        return Ok(None);
    }
    let query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND (serialized_name = ?1 OR serialized_name LIKE ?2 OR serialized_name LIKE ?3)
         LIMIT 1",
        kind_clause
    );
    conn.query_row(&query, params![exact, suffix_dot, suffix_colon], |row| {
        row.get(0)
    })
    .optional()
    .map_err(Into::into)
}

fn find_fuzzy(
    conn: &rusqlite::Connection,
    kind_clause: &str,
    exact: &str,
    suffix_dot: &str,
    suffix_colon: &str,
) -> Result<Option<i64>> {
    let fuzzy = format!("%{}%", exact);
    let query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND (serialized_name = ?1 OR serialized_name LIKE ?2 OR serialized_name LIKE ?3 OR serialized_name LIKE ?4)
         ORDER BY start_line LIMIT 1",
        kind_clause
    );
    conn.query_row(
        &query,
        params![exact, suffix_dot, suffix_colon, fuzzy],
        |row| row.get(0),
    )
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
    let query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND (serialized_name = ?1 OR serialized_name LIKE ?2 OR serialized_name LIKE ?3)
         ORDER BY start_line
         LIMIT {}",
        kind_clause, limit
    );
    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map(params![exact, suffix_dot, suffix_colon], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<i64>>>()?)
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
        "clear",
        "dedup",
        "extend",
        "insert",
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

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim(),
            "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
        ),
        Err(_) => default,
    }
}
