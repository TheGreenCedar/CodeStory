use super::*;

pub(super) fn cleanup_stale_call_resolutions(
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
             AND target_node_id IN (SELECT id FROM node WHERE lower(serialized_name) IN ({}))",
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
             AND target_node_id IN (SELECT id FROM node WHERE lower(serialized_name) IN ({}))
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

pub(super) fn unresolved_edges(
    conn: &rusqlite::Connection,
    kind: EdgeKind,
    scope_context: &ScopeCallerContext,
) -> Result<Vec<UnresolvedEdgeRow>> {
    if scope_context.is_empty() {
        return Ok(Vec::new());
    }

    let mut query = String::from(
        "SELECT e.id, caller.file_node_id, caller.qualified_name, caller.serialized_name, target.serialized_name, file_node.serialized_name, e.callsite_identity
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

pub(super) fn apply_resolution_updates(
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

pub(super) fn numbered_placeholders(start: usize, count: usize) -> String {
    (0..count)
        .map(|offset| format!("?{}", start + offset))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn question_placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

fn map_unresolved_edge_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UnresolvedEdgeRow> {
    Ok((
        row.get::<_, i64>(0)?,
        row.get::<_, Option<i64>>(1)?,
        row.get::<_, Option<String>>(2)?,
        row.get::<_, String>(3)?,
        row.get::<_, String>(4)?,
        row.get::<_, Option<String>>(5)?,
        row.get::<_, Option<String>>(6)?,
    ))
}
