use anyhow::Result;
use codestory_core::{EdgeKind, NodeKind};
use codestory_storage::Storage;
use rusqlite::{OptionalExtension, params};

type UnresolvedEdgeRow = (i64, Option<i64>, Option<String>, String);

#[derive(Default, Debug)]
pub struct ResolutionStats {
    pub resolved_calls: usize,
    pub unresolved_calls: usize,
    pub resolved_imports: usize,
    pub unresolved_imports: usize,
}

pub struct ResolutionPass;

impl Default for ResolutionPass {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolutionPass {
    pub fn new() -> Self {
        Self
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

        // Repair/avoid incorrect call resolutions:
        // - Previous versions used fuzzy matching for CALL edges, which can incorrectly resolve
        //   common method calls (e.g. `dedup`, `sort`) to unrelated project functions.
        // - Even with exact matching, some unqualified method names (e.g. `push`, `add`) are so
        //   common in stdlib/3rd-party types that resolving them by name alone is often wrong.
        //
        // For CALL edges we prefer leaving the target unresolved over linking to the wrong node,
        // since incorrect edges pollute the trail graph as depth increases.
        conn.execute(
            "UPDATE edge SET resolved_target_node_id = NULL, confidence = NULL
             WHERE kind = ?1 AND confidence IS NOT NULL AND confidence <= ?2",
            params![EdgeKind::CALL as i32, 0.4f32],
        )?;
        for common_name in common_unqualified_call_names() {
            conn.execute(
                "UPDATE edge SET resolved_target_node_id = NULL, confidence = NULL
                 WHERE kind = ?1
                 AND resolved_target_node_id IS NOT NULL
                 AND target_node_id IN (SELECT id FROM node WHERE serialized_name = ?2)",
                params![EdgeKind::CALL as i32, common_name],
            )?;
        }

        let mut resolved = 0usize;
        let rows = unresolved_edges(conn, EdgeKind::CALL)?;

        let function_kinds = [
            NodeKind::FUNCTION as i32,
            NodeKind::METHOD as i32,
            NodeKind::MACRO as i32,
        ];

        for (edge_id, file_id, caller_qualified, target_name) in rows {
            let is_common_unqualified = is_common_unqualified_call_name(&target_name);
            let (exact, suffix_dot, suffix_colon) = name_patterns(&target_name);

            // For very common unqualified method names, resolving within the same file is too
            // error-prone (e.g. `Vec::push` getting linked to an unrelated `SomeType::push`).
            // Prefer module/type-local resolution instead (see `find_same_module` below).
            if !is_common_unqualified
                && let Some(candidate) = find_same_file(
                    conn,
                    &function_kinds,
                    file_id,
                    &exact,
                    &suffix_dot,
                    &suffix_colon,
                )?
            {
                resolved += update_edge_resolution(conn, edge_id, candidate, 0.95)?;
                continue;
            }

            if let Some(prefix) = caller_qualified.and_then(module_prefix)
                && let Some(candidate) = find_same_module(
                    conn,
                    &function_kinds,
                    &prefix.0,
                    prefix.1,
                    &exact,
                    &suffix_dot,
                    &suffix_colon,
                )?
            {
                resolved += update_edge_resolution(conn, edge_id, candidate, 0.8)?;
                continue;
            }

            // For common unqualified method names (e.g. `push`, `add`), avoid global matching.
            // These calls are often to stdlib/3rd-party types where we cannot infer the receiver
            // type, and resolving by name alone produces incorrect edges.
            if is_common_unqualified {
                continue;
            }

            if let Some(candidate) =
                find_global_unique(conn, &function_kinds, &exact, &suffix_dot, &suffix_colon)?
            {
                resolved += update_edge_resolution(conn, edge_id, candidate, 0.6)?;
                continue;
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

        for (edge_id, file_id, caller_qualified, target_name) in rows {
            let (exact, suffix_dot, suffix_colon) = name_patterns(&target_name);

            if let Some(candidate) = find_same_file(
                conn,
                &module_kinds,
                file_id,
                &exact,
                &suffix_dot,
                &suffix_colon,
            )? {
                resolved += update_edge_resolution(conn, edge_id, candidate, 0.9)?;
                continue;
            }

            if let Some(prefix) = caller_qualified.and_then(module_prefix)
                && let Some(candidate) = find_same_module(
                    conn,
                    &module_kinds,
                    &prefix.0,
                    prefix.1,
                    &exact,
                    &suffix_dot,
                    &suffix_colon,
                )?
            {
                resolved += update_edge_resolution(conn, edge_id, candidate, 0.7)?;
                continue;
            }

            if let Some(candidate) =
                find_global_unique(conn, &module_kinds, &exact, &suffix_dot, &suffix_colon)?
            {
                resolved += update_edge_resolution(conn, edge_id, candidate, 0.5)?;
                continue;
            }

            if let Some(candidate) =
                find_fuzzy(conn, &module_kinds, &exact, &suffix_dot, &suffix_colon)?
            {
                resolved += update_edge_resolution(conn, edge_id, candidate, 0.3)?;
            }
        }

        Ok(resolved)
    }
}

fn update_edge_resolution(
    conn: &rusqlite::Connection,
    edge_id: i64,
    resolved_target: i64,
    confidence: f32,
) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE edge SET resolved_target_node_id = ?1, confidence = ?2 WHERE id = ?3",
        params![resolved_target, confidence, edge_id],
    )?)
}

fn unresolved_edges(conn: &rusqlite::Connection, kind: EdgeKind) -> Result<Vec<UnresolvedEdgeRow>> {
    let mut stmt = conn.prepare(
        "SELECT e.id, caller.file_node_id, caller.qualified_name, target.serialized_name
         FROM edge e
         JOIN node caller ON caller.id = e.source_node_id
         JOIN node target ON target.id = e.target_node_id
         WHERE e.kind = ?1 AND e.resolved_target_node_id IS NULL",
    )?;

    let rows = stmt.query_map(params![kind as i32], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;

    let collected = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(collected)
}

fn name_patterns(name: &str) -> (String, String, String) {
    (
        name.to_string(),
        format!("%.{}", name),
        format!("%::{}", name),
    )
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
    kinds: &[i32],
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
        kind_clause(kinds)
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
    kinds: &[i32],
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
        kind_clause(kinds)
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
    kinds: &[i32],
    exact: &str,
    suffix_dot: &str,
    suffix_colon: &str,
) -> Result<Option<i64>> {
    let count_query = format!(
        "SELECT COUNT(*) FROM node
         WHERE kind IN ({})
         AND (serialized_name = ?1 OR serialized_name LIKE ?2 OR serialized_name LIKE ?3)",
        kind_clause(kinds)
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
        kind_clause(kinds)
    );
    conn.query_row(&query, params![exact, suffix_dot, suffix_colon], |row| {
        row.get(0)
    })
    .optional()
    .map_err(Into::into)
}

fn find_fuzzy(
    conn: &rusqlite::Connection,
    kinds: &[i32],
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
        kind_clause(kinds)
    );
    conn.query_row(
        &query,
        params![exact, suffix_dot, suffix_colon, fuzzy],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn kind_clause(kinds: &[i32]) -> String {
    kinds
        .iter()
        .map(|k| k.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn common_unqualified_call_names() -> &'static [&'static str] {
    // Keep this list intentionally small and focused on very common stdlib-like methods.
    // These are frequently called on collections/types where we can't infer the receiver type,
    // and resolving them by name alone is likely wrong.
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
    // If the target already looks qualified, allow normal resolution.
    if name.contains("::") || name.contains('.') {
        return false;
    }
    common_unqualified_call_names().contains(&name)
}
