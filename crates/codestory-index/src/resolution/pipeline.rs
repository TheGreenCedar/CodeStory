use super::*;

pub(super) fn resolve_calls_on_conn(
    pass: &ResolutionPass,
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
    sql::cleanup_stale_call_resolutions(conn, pass.flags, pass.policy, scope_context)?;
    telemetry.call_cleanup_ms = telemetry
        .call_cleanup_ms
        .saturating_add(duration_ms_u64(cleanup_started.elapsed()));

    resolve_edges_after_prepare(
        pass,
        conn,
        scope_context,
        EdgeKind::CALL,
        &[
            NodeKind::FUNCTION as i32,
            NodeKind::METHOD as i32,
            NodeKind::MACRO as i32,
        ],
        &mut telemetry.call_unresolved_load_ms,
        &mut telemetry.call_candidate_index_ms,
        &mut telemetry.call_compute_ms,
        &mut telemetry.call_apply_ms,
        strategy_counters,
        ResolutionPass::compute_call_resolution,
    )
}

pub(super) fn resolve_imports_on_conn(
    pass: &ResolutionPass,
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

    resolve_edges_after_prepare(
        pass,
        conn,
        scope_context,
        EdgeKind::IMPORT,
        &[
            NodeKind::MODULE as i32,
            NodeKind::NAMESPACE as i32,
            NodeKind::PACKAGE as i32,
        ],
        &mut telemetry.import_unresolved_load_ms,
        &mut telemetry.import_candidate_index_ms,
        &mut telemetry.import_compute_ms,
        &mut telemetry.import_apply_ms,
        strategy_counters,
        ResolutionPass::compute_import_resolution,
    )
}

fn resolve_edges_after_prepare<F>(
    pass: &ResolutionPass,
    conn: &rusqlite::Connection,
    scope_context: &ScopeCallerContext,
    edge_kind: EdgeKind,
    candidate_kinds: &[i32],
    unresolved_load_ms: &mut u64,
    candidate_index_ms: &mut u64,
    compute_ms: &mut u64,
    apply_ms: &mut u64,
    strategy_counters: &mut ResolutionStrategyCounters,
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
    let rows = sql::unresolved_edges(conn, edge_kind, scope_context)?;
    *unresolved_load_ms = unresolved_load_ms.saturating_add(duration_ms_u64(rows_started.elapsed()));
    if rows.is_empty() {
        return Ok(0);
    }

    let candidate_started = Instant::now();
    let candidate_index = CandidateIndex::load(conn, candidate_kinds)?;
    *candidate_index_ms =
        candidate_index_ms.saturating_add(duration_ms_u64(candidate_started.elapsed()));

    let semantic_candidates_by_row = pass.semantic_candidates_for_rows(conn, &rows, edge_kind)?;

    let compute_started = Instant::now();
    let computed_results: Vec<Result<ComputedResolution>> =
        if pass.flags.parallel_compute && rows.len() > 1 {
            rows.par_iter()
                .zip(semantic_candidates_by_row.par_iter())
                .map(|(row, semantic_candidates)| compute(pass, &candidate_index, row, semantic_candidates))
                .collect()
        } else {
            rows.iter()
                .zip(semantic_candidates_by_row.iter())
                .map(|(row, semantic_candidates)| compute(pass, &candidate_index, row, semantic_candidates))
                .collect()
        };
    *compute_ms = compute_ms.saturating_add(duration_ms_u64(compute_started.elapsed()));

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
    sql::apply_resolution_updates(conn, &updates)?;
    *apply_ms = apply_ms.saturating_add(duration_ms_u64(apply_started.elapsed()));
    Ok(resolved)
}
