use super::*;

struct PreparedResolutionJob<'a> {
    edge_kind: EdgeKind,
    semantic_request_stats: &'a mut SemanticRequestStats,
    unresolved_load_ms: &'a mut u64,
    semantic_candidates_ms: &'a mut u64,
    compute_ms: &'a mut u64,
    apply_ms: &'a mut u64,
    strategy_counters: &'a mut ResolutionStrategyCounters,
}

pub(super) fn resolve_calls_on_conn(
    pass: &ResolutionPass,
    conn: &rusqlite::Connection,
    scope_context: &ScopeCallerContext,
    prepared: &PreparedResolutionState,
    telemetry: &mut ResolutionPhaseTelemetry,
    strategy_counters: &mut ResolutionStrategyCounters,
) -> Result<usize> {
    if scope_context.is_empty() {
        return Ok(0);
    }

    let prepare_started = Instant::now();
    let mut prepare_query = String::from(
        "UPDATE edge SET resolved_source_node_id = source_node_id
         WHERE kind = ?1 AND resolved_source_node_id IS NULL",
    );
    if scope_context.is_scoped() {
        prepare_query.push_str(&format!(
            " AND source_node_id IN (SELECT caller_id FROM {SCOPED_CALLER_TABLE})"
        ));
    }
    conn.execute(&prepare_query, params![EdgeKind::CALL as i32])?;
    telemetry.call_prepare_ms = telemetry
        .call_prepare_ms
        .saturating_add(duration_ms_u64(prepare_started.elapsed()));

    let cleanup_started = Instant::now();
    sql::cleanup_stale_call_resolutions(conn, pass.flags, pass.policy, scope_context)?;
    telemetry.call_cleanup_ms = telemetry
        .call_cleanup_ms
        .saturating_add(duration_ms_u64(cleanup_started.elapsed()));

    let mut semantic_request_stats = SemanticRequestStats::default();
    let resolved = resolve_edges_after_prepare(
        pass,
        conn,
        scope_context,
        &prepared.call_candidate_index,
        &prepared.call_semantic_index,
        PreparedResolutionJob {
            edge_kind: EdgeKind::CALL,
            semantic_request_stats: &mut semantic_request_stats,
            unresolved_load_ms: &mut telemetry.call_unresolved_load_ms,
            semantic_candidates_ms: &mut telemetry.call_semantic_candidates_ms,
            compute_ms: &mut telemetry.call_compute_ms,
            apply_ms: &mut telemetry.call_apply_ms,
            strategy_counters,
        },
        ResolutionPass::compute_call_resolution,
    )?;
    telemetry.record_semantic_request_stats(EdgeKind::CALL, semantic_request_stats);
    Ok(resolved)
}

pub(super) fn resolve_imports_on_conn(
    pass: &ResolutionPass,
    conn: &rusqlite::Connection,
    scope_context: &ScopeCallerContext,
    prepared: &PreparedResolutionState,
    telemetry: &mut ResolutionPhaseTelemetry,
    strategy_counters: &mut ResolutionStrategyCounters,
) -> Result<usize> {
    if scope_context.is_empty() {
        return Ok(0);
    }

    let prepare_started = Instant::now();
    let mut prepare_query = String::from(
        "UPDATE edge SET resolved_source_node_id = source_node_id
         WHERE kind = ?1 AND resolved_source_node_id IS NULL",
    );
    if scope_context.is_scoped() {
        prepare_query.push_str(&format!(
            " AND source_node_id IN (SELECT caller_id FROM {SCOPED_CALLER_TABLE})"
        ));
    }
    conn.execute(&prepare_query, params![EdgeKind::IMPORT as i32])?;
    telemetry.import_prepare_ms = telemetry
        .import_prepare_ms
        .saturating_add(duration_ms_u64(prepare_started.elapsed()));

    let mut semantic_request_stats = SemanticRequestStats::default();
    let resolved = resolve_edges_after_prepare(
        pass,
        conn,
        scope_context,
        &prepared.import_candidate_index,
        &prepared.import_semantic_index,
        PreparedResolutionJob {
            edge_kind: EdgeKind::IMPORT,
            semantic_request_stats: &mut semantic_request_stats,
            unresolved_load_ms: &mut telemetry.import_unresolved_load_ms,
            semantic_candidates_ms: &mut telemetry.import_semantic_candidates_ms,
            compute_ms: &mut telemetry.import_compute_ms,
            apply_ms: &mut telemetry.import_apply_ms,
            strategy_counters,
        },
        ResolutionPass::compute_import_resolution,
    )?;
    telemetry.record_semantic_request_stats(EdgeKind::IMPORT, semantic_request_stats);
    Ok(resolved)
}

pub(super) fn resolve_overrides_on_conn(
    pass: &ResolutionPass,
    conn: &rusqlite::Connection,
    scope_context: &ScopeCallerContext,
    prepared: &PreparedResolutionState,
    telemetry: &mut ResolutionPhaseTelemetry,
) -> Result<usize> {
    if scope_context.is_empty() {
        return Ok(0);
    }

    let override_started = Instant::now();
    let mut prepare_query = String::from(
        "UPDATE edge
         SET resolved_source_node_id = source_node_id,
             resolved_target_node_id = NULL,
             confidence = NULL,
             certainty = NULL,
             candidate_target_node_ids = NULL
         WHERE kind = ?1",
    );
    if scope_context.is_scoped() {
        prepare_query.push_str(&format!(
            " AND source_node_id IN (SELECT caller_id FROM {SCOPED_CALLER_TABLE})"
        ));
    }
    conn.execute(&prepare_query, params![EdgeKind::OVERRIDE as i32])?;

    let rows = unresolved_override_edges(conn, scope_context)?;
    if rows.is_empty() {
        return Ok(0);
    }

    let owner_by_method = &prepared.override_support.owner_by_method;
    let methods_by_owner_and_name = &prepared.override_support.methods_by_owner_and_name;
    let owner_name_by_id = &prepared.override_support.owner_name_by_id;
    let methods_by_owner_name_and_name = &prepared.override_support.methods_by_owner_name_and_name;
    let inheritance_by_type = &prepared.override_support.inheritance_by_type;
    let inheritance_by_owner_name = &prepared.override_support.inheritance_by_owner_name;
    let mut resolved = 0usize;
    let mut updates = Vec::with_capacity(rows.len());

    for (edge_id, source_id, source_name) in rows {
        let method_name = short_member_name(&source_name);
        if let Some(owner_name) = owner_name_from_member_name(&source_name) {
            let mut candidate_ids = collect_override_candidates_by_owner_name(
                owner_name,
                method_name,
                inheritance_by_owner_name,
                methods_by_owner_name_and_name,
            );
            candidate_ids.sort_unstable();
            candidate_ids.dedup();
            if candidate_ids.len() > 1 {
                let candidate_names = candidate_ids
                    .iter()
                    .filter_map(|candidate_id| owner_name_by_id.get(candidate_id).cloned())
                    .collect::<HashSet<_>>();
                if candidate_names.len() == 1 {
                    candidate_ids.truncate(1);
                }
            }
            let selected = (candidate_ids.len() == 1).then(|| (candidate_ids[0], 1.0_f32));
            if selected.is_some() {
                resolved += 1;
            }
            let candidate_slice = if pass.flags.store_candidates {
                candidate_ids.as_slice()
            } else {
                &[]
            };
            updates.push(build_resolved_edge_update(
                edge_id,
                selected,
                candidate_slice,
            )?);
            continue;
        }
        let Some(owner_ids) = owner_by_method.get(&source_id) else {
            updates.push(build_resolved_edge_update(edge_id, None, &[])?);
            continue;
        };
        let mut owner_ids = owner_ids.clone();
        owner_ids.sort_unstable();
        owner_ids.dedup();
        let owner_id = if owner_ids.len() == 1 {
            owner_ids[0]
        } else {
            let owner_names = owner_ids
                .iter()
                .filter_map(|owner_id| owner_name_by_id.get(owner_id).cloned())
                .collect::<HashSet<_>>();
            if owner_names.len() == 1 {
                owner_ids[0]
            } else {
                updates.push(build_resolved_edge_update(edge_id, None, &[])?);
                continue;
            }
        };
        if owner_id == 0 {
            updates.push(build_resolved_edge_update(edge_id, None, &[])?);
            continue;
        }

        let candidate_ids = collect_override_candidates(
            owner_id,
            method_name,
            inheritance_by_type,
            methods_by_owner_and_name,
            owner_name_by_id,
            methods_by_owner_name_and_name,
        );
        let mut candidate_ids = candidate_ids;
        if candidate_ids.len() > 1 {
            let candidate_names = candidate_ids
                .iter()
                .filter_map(|candidate_id| owner_name_by_id.get(candidate_id).cloned())
                .collect::<HashSet<_>>();
            if candidate_names.len() == 1 {
                candidate_ids.sort_unstable();
                candidate_ids.truncate(1);
            }
        }
        let selected = (candidate_ids.len() == 1).then(|| (candidate_ids[0], 1.0_f32));
        if selected.is_some() {
            resolved += 1;
        }
        let candidate_slice = if pass.flags.store_candidates {
            candidate_ids.as_slice()
        } else {
            &[]
        };
        updates.push(build_resolved_edge_update(
            edge_id,
            selected,
            candidate_slice,
        )?);
    }

    sql::apply_resolution_updates(conn, &updates)?;
    telemetry.override_resolution_ms = telemetry
        .override_resolution_ms
        .saturating_add(duration_ms_u64(override_started.elapsed()));
    Ok(resolved)
}

fn resolve_edges_after_prepare<F>(
    pass: &ResolutionPass,
    conn: &rusqlite::Connection,
    scope_context: &ScopeCallerContext,
    candidate_index: &CandidateIndex,
    semantic_index: &SemanticCandidateIndex,
    job: PreparedResolutionJob<'_>,
    compute: F,
) -> Result<usize>
where
    F: Fn(
            &ResolutionPass,
            &CandidateIndex,
            &UnresolvedEdgeRow,
            &[SemanticResolutionCandidate],
        ) -> Result<ComputedResolution>
        + Sync,
{
    let rows_started = Instant::now();
    let rows = sql::unresolved_edges(conn, job.edge_kind, scope_context)?;
    *job.unresolved_load_ms = job
        .unresolved_load_ms
        .saturating_add(duration_ms_u64(rows_started.elapsed()));
    if rows.is_empty() {
        return Ok(0);
    }

    let semantic_candidates_started = Instant::now();
    let (semantic_candidates_by_row, semantic_request_stats) =
        pass.semantic_candidates_for_rows(semantic_index, &rows, job.edge_kind)?;
    *job.semantic_request_stats = semantic_request_stats;
    *job.semantic_candidates_ms = job
        .semantic_candidates_ms
        .saturating_add(duration_ms_u64(semantic_candidates_started.elapsed()));

    let compute_started = Instant::now();
    let computed_results: Vec<Result<ComputedResolution>> =
        if pass.flags.parallel_compute && rows.len() > 1 {
            rows.par_iter()
                .zip(semantic_candidates_by_row.par_iter())
                .map(|(row, semantic_candidates)| {
                    compute(pass, candidate_index, row, semantic_candidates)
                })
                .collect()
        } else {
            rows.iter()
                .zip(semantic_candidates_by_row.iter())
                .map(|(row, semantic_candidates)| {
                    compute(pass, candidate_index, row, semantic_candidates)
                })
                .collect()
        };
    *job.compute_ms = job
        .compute_ms
        .saturating_add(duration_ms_u64(compute_started.elapsed()));

    let mut resolved = 0usize;
    let mut updates = Vec::with_capacity(rows.len());
    for computed in computed_results {
        let computed = computed?;
        if computed.strategy.is_some() {
            resolved += 1;
        }
        job.strategy_counters.record(computed.strategy);
        updates.push(computed.update);
    }

    let apply_started = Instant::now();
    sql::apply_resolution_updates(conn, &updates)?;
    *job.apply_ms = job
        .apply_ms
        .saturating_add(duration_ms_u64(apply_started.elapsed()));
    Ok(resolved)
}

fn unresolved_override_edges(
    conn: &rusqlite::Connection,
    scope_context: &ScopeCallerContext,
) -> Result<Vec<(i64, i64, String)>> {
    let mut query = String::from(
        "SELECT e.id, e.source_node_id, source.serialized_name
         FROM edge e
         JOIN node source ON source.id = e.source_node_id
         WHERE e.kind = ?1 AND e.resolved_target_node_id IS NULL",
    );
    if scope_context.is_scoped() {
        query.push_str(&format!(
            " AND e.source_node_id IN (SELECT caller_id FROM {SCOPED_CALLER_TABLE})"
        ));
    }
    query.push_str(" ORDER BY e.id");

    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map(params![EdgeKind::OVERRIDE as i32], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(super) fn load_override_support(conn: &rusqlite::Connection) -> Result<OverrideSupport> {
    let override_members = load_override_member_rows(conn)?;
    let override_inheritance = load_override_inheritance_rows(conn)?;
    let override_inheritance_by_name = load_override_inheritance_by_name_rows(conn)?;
    let node_names = load_node_name_rows(conn)?;
    Ok(OverrideSupport::from_snapshot(
        override_members,
        override_inheritance,
        override_inheritance_by_name,
        node_names,
    ))
}

fn load_override_member_rows(conn: &rusqlite::Connection) -> Result<Vec<OverrideMemberSnapshot>> {
    let mut stmt = conn.prepare(
        "SELECT member.source_node_id, owner.serialized_name, member.target_node_id, method.serialized_name
         FROM edge member
         JOIN node owner ON owner.id = member.source_node_id
         JOIN node method ON method.id = member.target_node_id
         WHERE member.kind = ?1 AND method.kind = ?2",
    )?;
    let rows = stmt.query_map(
        params![EdgeKind::MEMBER as i32, NodeKind::METHOD as i32],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (owner_id, owner_name, method_id, serialized_name) = row?;
        out.push(OverrideMemberSnapshot {
            owner_id,
            owner_name,
            method_id,
            method_name: short_member_name(&serialized_name).to_string(),
        });
    }
    Ok(out)
}

fn load_override_inheritance_rows(conn: &rusqlite::Connection) -> Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT source_node_id, COALESCE(resolved_target_node_id, target_node_id)
         FROM edge
         WHERE kind = ?1",
    )?;
    let rows = stmt.query_map(params![EdgeKind::INHERITANCE as i32], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn load_override_inheritance_by_name_rows(
    conn: &rusqlite::Connection,
) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT source.serialized_name, target.serialized_name
         FROM edge inheritance
         JOIN node source ON source.id = inheritance.source_node_id
         JOIN node target ON target.id = COALESCE(inheritance.resolved_target_node_id, inheritance.target_node_id)
         WHERE inheritance.kind = ?1",
    )?;
    let rows = stmt.query_map(params![EdgeKind::INHERITANCE as i32], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn load_node_name_rows(conn: &rusqlite::Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare("SELECT id, serialized_name FROM node")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn collect_override_candidates(
    owner_id: i64,
    method_name: &str,
    inheritance_by_type: &HashMap<i64, Vec<i64>>,
    methods_by_owner_and_name: &HashMap<(i64, String), Vec<i64>>,
    owner_name_by_id: &HashMap<i64, String>,
    methods_by_owner_name_and_name: &HashMap<(String, String), Vec<i64>>,
) -> Vec<i64> {
    let mut pending = std::collections::VecDeque::from([owner_id]);
    let mut visited = HashSet::new();
    let mut candidates = OrderedCandidateIds::default();

    while let Some(current_owner) = pending.pop_front() {
        if !visited.insert(current_owner) {
            continue;
        }
        if current_owner != owner_id {
            if let Some(method_ids) =
                methods_by_owner_and_name.get(&(current_owner, method_name.to_string()))
            {
                candidates.extend_stage(method_ids, usize::MAX);
            }
            if let Some(owner_name) = owner_name_by_id.get(&current_owner)
                && let Some(method_ids) = methods_by_owner_name_and_name
                    .get(&(owner_name.clone(), method_name.to_string()))
            {
                candidates.extend_stage(method_ids, usize::MAX);
            }
        }
        if let Some(parents) = inheritance_by_type.get(&current_owner) {
            for parent in parents {
                pending.push_back(*parent);
            }
        }
    }

    candidates.into_vec()
}

fn collect_override_candidates_by_owner_name(
    owner_name: &str,
    method_name: &str,
    inheritance_by_owner_name: &HashMap<String, Vec<String>>,
    methods_by_owner_name_and_name: &HashMap<(String, String), Vec<i64>>,
) -> Vec<i64> {
    let mut pending = std::collections::VecDeque::from([owner_name.to_string()]);
    let mut visited = HashSet::new();
    let mut candidates = OrderedCandidateIds::default();
    let method_name = method_name.to_string();

    while let Some(current_owner) = pending.pop_front() {
        if !visited.insert(current_owner.clone()) {
            continue;
        }
        if current_owner != owner_name
            && let Some(method_ids) =
                methods_by_owner_name_and_name.get(&(current_owner.clone(), method_name.clone()))
        {
            candidates.extend_stage(method_ids, usize::MAX);
        }
        if let Some(parents) = inheritance_by_owner_name.get(&current_owner) {
            for parent in parents {
                pending.push_back(parent.clone());
            }
        }
    }

    candidates.into_vec()
}

fn owner_name_from_member_name(name: &str) -> Option<&str> {
    let colon = name.rfind("::");
    let dot = name.rfind('.');
    match (colon, dot) {
        (Some(colon_idx), Some(dot_idx)) => {
            let split = if colon_idx + 1 > dot_idx {
                colon_idx
            } else {
                dot_idx
            };
            Some(&name[..split])
        }
        (Some(colon_idx), None) => Some(&name[..colon_idx]),
        (None, Some(dot_idx)) => Some(&name[..dot_idx]),
        (None, None) => None,
    }
}

fn short_member_name(name: &str) -> &str {
    let colon = name.rfind("::").map(|idx| idx + 2).unwrap_or(0);
    let dot = name.rfind('.').map(|idx| idx + 1).unwrap_or(0);
    let split = colon.max(dot);
    &name[split..]
}
