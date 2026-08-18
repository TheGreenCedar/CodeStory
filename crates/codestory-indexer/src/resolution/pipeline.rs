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
    cancel_token: Option<&CancellationToken>,
) -> Result<usize> {
    ResolutionPass::check_cancelled(cancel_token)?;
    if scope_context.is_empty() {
        return Ok(0);
    }

    ResolutionPass::check_cancelled(cancel_token)?;
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

    ResolutionPass::check_cancelled(cancel_token)?;
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
        cancel_token,
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
    cancel_token: Option<&CancellationToken>,
) -> Result<usize> {
    ResolutionPass::check_cancelled(cancel_token)?;
    if scope_context.is_empty() {
        return Ok(0);
    }

    ResolutionPass::check_cancelled(cancel_token)?;
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

    ResolutionPass::check_cancelled(cancel_token)?;
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
        cancel_token,
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
    cancel_token: Option<&CancellationToken>,
) -> Result<usize> {
    ResolutionPass::check_cancelled(cancel_token)?;
    if scope_context.is_empty() {
        return Ok(0);
    }

    ResolutionPass::check_cancelled(cancel_token)?;
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

    ResolutionPass::check_cancelled(cancel_token)?;
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
        ResolutionPass::check_cancelled(cancel_token)?;
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

    ResolutionPass::check_cancelled(cancel_token)?;
    sql::apply_resolution_updates(conn, &updates)?;
    telemetry.override_resolution_ms = telemetry
        .override_resolution_ms
        .saturating_add(duration_ms_u64(override_started.elapsed()));
    Ok(resolved)
}

/// Resolve structural CSS reference placeholders (stage 3c, P3.i/P3.v).
///
/// The CSS collector mints three placeholder families it cannot resolve at
/// emit time, all `NodeKind::UNKNOWN` synthetic nodes:
///
/// - `css:import-ref:{resolved path}` — the raw target of a file→file IMPORT
///   edge. This job stamps `resolved_target_node_id` with the canonical file
///   node id derived from that importer-directory-resolved path — identity
///   derivation plus an existence check against a real FILE node, never name
///   matching. The raw target deliberately is not a locally minted FILE node
///   carrying the canonical id: store nodes upsert last-write-wins by id, so
///   a placeholder FILE row could clobber the real file node's projection.
/// - `css:var-ref:{--name}` / `css:keyframes-ref:{name}` — targets of
///   selector USAGE edges whose declaration is not in the same file. These
///   resolve against `css:var:` VARIABLE / `css:keyframes:` FUNCTION
///   declarations in the referencing file's IMPORT-GRAPH COMPONENT: the BFS
///   traverses file→file IMPORT effective endpoints in BOTH directions
///   (imports and importers, transitively). Custom properties resolve
///   through the page cascade — a `var()` in one sheet is satisfied by a
///   declaration in any sheet loaded with it — and structurally that is the
///   connected component, not the downstream closure: in animate.css,
///   `_base.css` imports nothing and reaches `_vars.css` only through their
///   shared importer (`_base.css` ← `animate.css` → `_vars.css`). A name
///   declared in zero or in more than one component file stays unresolved.
///
/// Bounded and deterministic: the worklist is exactly the unresolved
/// placeholder-target edges ordered by edge id; the component walk is a
/// plain BFS with a visited set over a finite file graph; ambiguity and
/// misses fail closed, leaving the UNKNOWN-kind placeholder as the effective
/// target, which can never satisfy a matcher that requires a VARIABLE /
/// FUNCTION / FILE effective target.
///
/// Deliberately unscoped (beyond the empty-scope early return): a FullReplace
/// of a declaration file NULLs the resolutions of surviving edges in OTHER
/// files (`delete_file_projection`), and those files are outside the
/// incremental caller scope; re-attempting the tiny placeholder worklist each
/// pass is what keeps cross-file references fresh.
pub(super) fn resolve_structural_css_references_on_conn(
    conn: &rusqlite::Connection,
    scope_context: &ScopeCallerContext,
    cancel_token: Option<&CancellationToken>,
) -> Result<usize> {
    ResolutionPass::check_cancelled(cancel_token)?;
    if scope_context.is_empty() {
        return Ok(0);
    }

    let mut resolved = 0usize;
    let mut update = conn.prepare(
        "UPDATE edge
         SET resolved_source_node_id = source_node_id,
             resolved_target_node_id = ?2,
             confidence = 1.0,
             certainty = ?3
         WHERE id = ?1",
    )?;
    let certain = ResolutionCertainty::Certain.as_str();

    // Import placeholders: canonical-file-id derivation + FILE existence.
    let import_rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT e.id, COALESCE(target.qualified_name, target.serialized_name)
             FROM edge e
             JOIN node target ON target.id = e.target_node_id
             WHERE e.kind = ?1 AND e.resolved_target_node_id IS NULL
               AND target.canonical_id LIKE 'css:import-ref:%'
             ORDER BY e.id",
        )?;
        let rows = stmt.query_map(params![EdgeKind::IMPORT as i32], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut file_exists = conn.prepare("SELECT COUNT(*) FROM node WHERE id = ?1 AND kind = ?2")?;
    for (edge_id, resolved_path) in import_rows {
        ResolutionPass::check_cancelled(cancel_token)?;
        let candidate_id = crate::WorkspaceIndexer::canonical_file_node_id_for_path(
            std::path::Path::new(&resolved_path),
        );
        let exists: i64 = file_exists
            .query_row(params![candidate_id, NodeKind::FILE as i32], |row| {
                row.get(0)
            })?;
        if exists == 0 {
            continue;
        }
        update.execute(params![edge_id, candidate_id, certain])?;
        resolved += 1;
    }

    // Selector USAGE placeholders: declarations in the import-graph
    // component (cascade semantics — see the function doc).
    let usage_rows: Vec<(i64, Option<i64>, String)> = {
        let mut stmt = conn.prepare(
            "SELECT e.id, e.file_node_id, target.canonical_id
             FROM edge e
             JOIN node target ON target.id = e.target_node_id
             WHERE e.kind = ?1 AND e.resolved_target_node_id IS NULL
               AND (target.canonical_id LIKE 'css:var-ref:%'
                    OR target.canonical_id LIKE 'css:keyframes-ref:%')
             ORDER BY e.id",
        )?;
        let rows = stmt.query_map(params![EdgeKind::USAGE as i32], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if usage_rows.is_empty() {
        return Ok(resolved);
    }

    let mut import_adjacency: HashMap<i64, Vec<i64>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT e.source_node_id,
                    COALESCE(e.resolved_target_node_id, e.target_node_id)
             FROM edge e
             JOIN node s ON s.id = e.source_node_id AND s.kind = ?2
             JOIN node t ON t.id = COALESCE(e.resolved_target_node_id, e.target_node_id)
                        AND t.kind = ?2
             WHERE e.kind = ?1
             ORDER BY e.source_node_id, t.id",
        )?;
        let rows = stmt.query_map(
            params![EdgeKind::IMPORT as i32, NodeKind::FILE as i32],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?;
        for row in rows {
            let (source, target) = row?;
            // Undirected on purpose: sibling sheets under a shared importer
            // are loaded into the same cascade, so the walk must reach a
            // declaration through the importer as well as through imports.
            import_adjacency.entry(source).or_default().push(target);
            import_adjacency.entry(target).or_default().push(source);
        }
    }

    // Declarations keyed by (reference canonical) so the lookup is a direct
    // string-identity join between `css:var-ref:X` and `css:var:X`.
    let mut declarations: HashMap<String, Vec<(i64, u32, i64)>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT n.id, n.file_node_id, n.canonical_id, COALESCE(n.start_line, 0)
             FROM node n
             WHERE (n.kind = ?1 AND n.canonical_id LIKE 'css:var:%')
                OR (n.kind = ?2 AND n.canonical_id LIKE 'css:keyframes:%')
             ORDER BY n.id",
        )?;
        let rows = stmt.query_map(
            params![NodeKind::VARIABLE as i32, NodeKind::FUNCTION as i32],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u32>(3)?,
                ))
            },
        )?;
        for row in rows {
            let (node_id, file_id, canonical, start_line) = row?;
            let Some(file_id) = file_id else { continue };
            let reference_canonical = if let Some(name) = canonical.strip_prefix("css:var:") {
                format!("css:var-ref:{name}")
            } else if let Some(name) = canonical.strip_prefix("css:keyframes:") {
                format!("css:keyframes-ref:{name}")
            } else {
                continue;
            };
            declarations
                .entry(reference_canonical)
                .or_default()
                .push((file_id, start_line, node_id));
        }
    }

    for (edge_id, file_id, reference_canonical) in usage_rows {
        ResolutionPass::check_cancelled(cancel_token)?;
        let Some(file_id) = file_id else { continue };
        let Some(candidates) = declarations.get(&reference_canonical) else {
            continue;
        };
        let mut component = HashSet::from([file_id]);
        let mut pending = std::collections::VecDeque::from([file_id]);
        while let Some(current) = pending.pop_front() {
            for neighbor in import_adjacency.get(&current).into_iter().flatten() {
                if component.insert(*neighbor) {
                    pending.push_back(*neighbor);
                }
            }
        }
        let mut in_component: Vec<&(i64, u32, i64)> = candidates
            .iter()
            .filter(|(declaring_file, _, _)| component.contains(declaring_file))
            .collect();
        let declaring_files: HashSet<i64> = in_component
            .iter()
            .map(|(declaring_file, _, _)| *declaring_file)
            .collect();
        if declaring_files.len() != 1 {
            // Zero or ambiguous declarations: the edge stays unresolved and
            // its UNKNOWN placeholder target keeps it un-dischargeable.
            continue;
        }
        in_component.sort_by_key(|(_, start_line, node_id)| (*start_line, *node_id));
        let (_, _, declaration_node_id) = in_component[0];
        update.execute(params![edge_id, declaration_node_id, certain])?;
        resolved += 1;
    }

    Ok(resolved)
}

#[allow(clippy::too_many_arguments)]
fn resolve_edges_after_prepare<F>(
    pass: &ResolutionPass,
    conn: &rusqlite::Connection,
    scope_context: &ScopeCallerContext,
    candidate_index: &CandidateIndex,
    semantic_index: &SemanticCandidateIndex,
    job: PreparedResolutionJob<'_>,
    compute: F,
    cancel_token: Option<&CancellationToken>,
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
    ResolutionPass::check_cancelled(cancel_token)?;
    let rows_started = Instant::now();
    let rows = sql::unresolved_edges(conn, job.edge_kind, scope_context)?;
    *job.unresolved_load_ms = job
        .unresolved_load_ms
        .saturating_add(duration_ms_u64(rows_started.elapsed()));
    if rows.is_empty() {
        return Ok(0);
    }

    ResolutionPass::check_cancelled(cancel_token)?;
    let semantic_candidates_started = Instant::now();
    let (semantic_candidates_by_row, semantic_request_stats) =
        pass.semantic_candidates_for_rows(semantic_index, &rows, job.edge_kind)?;
    *job.semantic_request_stats = semantic_request_stats;
    *job.semantic_candidates_ms = job
        .semantic_candidates_ms
        .saturating_add(duration_ms_u64(semantic_candidates_started.elapsed()));

    ResolutionPass::check_cancelled(cancel_token)?;
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

    ResolutionPass::check_cancelled(cancel_token)?;
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

    ResolutionPass::check_cancelled(cancel_token)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, params};

    fn css_reference_schema() -> Result<Connection> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE node (
                id INTEGER PRIMARY KEY,
                kind INTEGER NOT NULL,
                serialized_name TEXT NOT NULL,
                qualified_name TEXT,
                canonical_id TEXT,
                file_node_id INTEGER,
                start_line INTEGER
            );
            CREATE TABLE edge (
                id INTEGER PRIMARY KEY,
                kind INTEGER NOT NULL,
                source_node_id INTEGER NOT NULL,
                target_node_id INTEGER NOT NULL,
                file_node_id INTEGER,
                resolved_source_node_id INTEGER,
                resolved_target_node_id INTEGER,
                confidence REAL,
                certainty TEXT,
                candidate_target_node_ids TEXT,
                callsite_identity TEXT
            );",
        )?;
        Ok(conn)
    }

    fn insert_node(
        conn: &Connection,
        id: i64,
        kind: NodeKind,
        name: &str,
        canonical: Option<&str>,
        file_node_id: Option<i64>,
        start_line: Option<i64>,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, canonical_id, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6)",
            params![id, kind as i32, name, canonical, file_node_id, start_line],
        )?;
        Ok(())
    }

    fn insert_edge(
        conn: &Connection,
        id: i64,
        kind: EdgeKind,
        source: i64,
        target: i64,
        file_node_id: Option<i64>,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO edge (id, kind, source_node_id, target_node_id, file_node_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, kind as i32, source, target, file_node_id],
        )?;
        Ok(())
    }

    fn resolved_target(conn: &Connection, edge_id: i64) -> Result<Option<i64>> {
        Ok(conn.query_row(
            "SELECT resolved_target_node_id FROM edge WHERE id = ?1",
            params![edge_id],
            |row| row.get(0),
        )?)
    }

    #[test]
    fn css_var_references_resolve_only_through_an_unambiguous_import_closure() -> Result<()> {
        let conn = css_reference_schema()?;
        // Files: 10 imports 20 and 30; 40 stays outside the closure.
        for file_id in [10_i64, 20, 30, 40] {
            insert_node(&conn, file_id, NodeKind::FILE, "file", None, None, Some(1))?;
        }
        insert_edge(&conn, 100, EdgeKind::IMPORT, 10, 20, Some(10))?;
        conn.execute(
            "UPDATE edge SET resolved_target_node_id = 20 WHERE id = 100",
            [],
        )?;
        insert_edge(&conn, 101, EdgeKind::IMPORT, 10, 30, Some(10))?;
        conn.execute(
            "UPDATE edge SET resolved_target_node_id = 30 WHERE id = 101",
            [],
        )?;

        // Selector + placeholders in file 10.
        insert_node(
            &conn,
            1,
            NodeKind::CONSTANT,
            "app",
            Some("css:class:app"),
            Some(10),
            Some(3),
        )?;
        insert_node(
            &conn,
            2,
            NodeKind::UNKNOWN,
            "--unique",
            Some("css:var-ref:--unique"),
            Some(10),
            None,
        )?;
        insert_node(
            &conn,
            3,
            NodeKind::UNKNOWN,
            "--ambiguous",
            Some("css:var-ref:--ambiguous"),
            Some(10),
            None,
        )?;
        insert_node(
            &conn,
            4,
            NodeKind::UNKNOWN,
            "--outside",
            Some("css:var-ref:--outside"),
            Some(10),
            None,
        )?;
        insert_node(
            &conn,
            5,
            NodeKind::UNKNOWN,
            "spin",
            Some("css:keyframes-ref:spin"),
            Some(10),
            None,
        )?;

        // Declarations: --unique only in 20; --ambiguous in 20 AND 30;
        // --outside only in the non-imported 40; keyframes spin in 30.
        insert_node(
            &conn,
            6,
            NodeKind::VARIABLE,
            "--unique",
            Some("css:var:--unique"),
            Some(20),
            Some(2),
        )?;
        insert_node(
            &conn,
            7,
            NodeKind::VARIABLE,
            "--ambiguous",
            Some("css:var:--ambiguous"),
            Some(20),
            Some(3),
        )?;
        insert_node(
            &conn,
            8,
            NodeKind::VARIABLE,
            "--ambiguous",
            Some("css:var:--ambiguous"),
            Some(30),
            Some(1),
        )?;
        insert_node(
            &conn,
            9,
            NodeKind::VARIABLE,
            "--outside",
            Some("css:var:--outside"),
            Some(40),
            Some(1),
        )?;
        insert_node(
            &conn,
            11,
            NodeKind::FUNCTION,
            "spin",
            Some("css:keyframes:spin"),
            Some(30),
            Some(4),
        )?;

        insert_edge(&conn, 200, EdgeKind::USAGE, 1, 2, Some(10))?;
        insert_edge(&conn, 201, EdgeKind::USAGE, 1, 3, Some(10))?;
        insert_edge(&conn, 202, EdgeKind::USAGE, 1, 4, Some(10))?;
        insert_edge(&conn, 203, EdgeKind::USAGE, 1, 5, Some(10))?;

        let scope_context = ScopeCallerContext::prepare(&conn, None)?;
        let resolved = resolve_structural_css_references_on_conn(&conn, &scope_context, None)?;
        assert_eq!(resolved, 2, "only the unambiguous in-closure references");

        assert_eq!(resolved_target(&conn, 200)?, Some(6));
        assert_eq!(
            resolved_target(&conn, 201)?,
            None,
            "a name declared in two closure files stays unresolved"
        );
        assert_eq!(
            resolved_target(&conn, 202)?,
            None,
            "a declaration outside the import closure never resolves"
        );
        assert_eq!(resolved_target(&conn, 203)?, Some(11));

        // Re-running is idempotent: the worklist is already drained.
        let resolved_again =
            resolve_structural_css_references_on_conn(&conn, &scope_context, None)?;
        assert_eq!(resolved_again, 0);
        Ok(())
    }

    #[test]
    fn css_sibling_sheets_resolve_only_unambiguous_component_declarations() -> Result<()> {
        let conn = css_reference_schema()?;
        // Cascade component: parent 10 imports siblings 20, 30, and 40. The
        // references live in 30, which imports NOTHING itself — every
        // declaration is reachable only through the shared importer. File 50
        // is disconnected from the component entirely.
        for file_id in [10_i64, 20, 30, 40, 50] {
            insert_node(&conn, file_id, NodeKind::FILE, "file", None, None, Some(1))?;
        }
        insert_edge(&conn, 100, EdgeKind::IMPORT, 10, 20, Some(10))?;
        insert_edge(&conn, 101, EdgeKind::IMPORT, 10, 30, Some(10))?;
        insert_edge(&conn, 102, EdgeKind::IMPORT, 10, 40, Some(10))?;

        // Selector + placeholders in the import-less sibling 30.
        insert_node(
            &conn,
            1,
            NodeKind::CONSTANT,
            "animated",
            Some("css:class:animated"),
            Some(30),
            Some(1),
        )?;
        insert_node(
            &conn,
            2,
            NodeKind::UNKNOWN,
            "--sibling",
            Some("css:var-ref:--sibling"),
            Some(30),
            None,
        )?;
        insert_node(
            &conn,
            3,
            NodeKind::UNKNOWN,
            "--twice",
            Some("css:var-ref:--twice"),
            Some(30),
            None,
        )?;
        insert_node(
            &conn,
            4,
            NodeKind::UNKNOWN,
            "--disconnected",
            Some("css:var-ref:--disconnected"),
            Some(30),
            None,
        )?;
        insert_node(
            &conn,
            5,
            NodeKind::UNKNOWN,
            "wobble",
            Some("css:keyframes-ref:wobble"),
            Some(30),
            None,
        )?;

        // Declarations: --sibling and keyframes wobble only in sibling 20;
        // --twice in TWO sibling files (20 and 40); --disconnected only in
        // the unconnected 50.
        insert_node(
            &conn,
            6,
            NodeKind::VARIABLE,
            "--sibling",
            Some("css:var:--sibling"),
            Some(20),
            Some(2),
        )?;
        insert_node(
            &conn,
            7,
            NodeKind::FUNCTION,
            "wobble",
            Some("css:keyframes:wobble"),
            Some(20),
            Some(5),
        )?;
        insert_node(
            &conn,
            8,
            NodeKind::VARIABLE,
            "--twice",
            Some("css:var:--twice"),
            Some(20),
            Some(3),
        )?;
        insert_node(
            &conn,
            9,
            NodeKind::VARIABLE,
            "--twice",
            Some("css:var:--twice"),
            Some(40),
            Some(1),
        )?;
        insert_node(
            &conn,
            11,
            NodeKind::VARIABLE,
            "--disconnected",
            Some("css:var:--disconnected"),
            Some(50),
            Some(1),
        )?;

        insert_edge(&conn, 200, EdgeKind::USAGE, 1, 2, Some(30))?;
        insert_edge(&conn, 201, EdgeKind::USAGE, 1, 3, Some(30))?;
        insert_edge(&conn, 202, EdgeKind::USAGE, 1, 4, Some(30))?;
        insert_edge(&conn, 203, EdgeKind::USAGE, 1, 5, Some(30))?;

        let scope_context = ScopeCallerContext::prepare(&conn, None)?;
        let resolved = resolve_structural_css_references_on_conn(&conn, &scope_context, None)?;
        assert_eq!(resolved, 2, "only the unambiguous in-component references");

        assert_eq!(
            resolved_target(&conn, 200)?,
            Some(6),
            "a sibling declaration resolves through the shared importer"
        );
        assert_eq!(
            resolved_target(&conn, 203)?,
            Some(7),
            "a sibling keyframe declaration resolves through the shared importer"
        );
        assert_eq!(
            resolved_target(&conn, 201)?,
            None,
            "a name declared in two sibling files is ambiguous and stays unresolved"
        );
        assert_eq!(
            resolved_target(&conn, 202)?,
            None,
            "a declaration outside the import-graph component never resolves"
        );
        Ok(())
    }

    #[test]
    fn css_import_references_stamp_only_existing_canonical_file_nodes() -> Result<()> {
        let conn = css_reference_schema()?;
        insert_node(&conn, 10, NodeKind::FILE, "entry", None, None, Some(1))?;
        let vars_path = "/repo/theme/vars.css";
        let vars_file_id = crate::WorkspaceIndexer::canonical_file_node_id_for_path(
            std::path::Path::new(vars_path),
        );
        insert_node(
            &conn,
            vars_file_id,
            NodeKind::FILE,
            vars_path,
            None,
            None,
            Some(1),
        )?;
        insert_node(
            &conn,
            2,
            NodeKind::UNKNOWN,
            vars_path,
            Some("css:import-ref:/repo/theme/vars.css"),
            Some(10),
            None,
        )?;
        insert_node(
            &conn,
            3,
            NodeKind::UNKNOWN,
            "/repo/theme/missing.css",
            Some("css:import-ref:/repo/theme/missing.css"),
            Some(10),
            None,
        )?;
        insert_edge(&conn, 100, EdgeKind::IMPORT, 10, 2, Some(10))?;
        insert_edge(&conn, 101, EdgeKind::IMPORT, 10, 3, Some(10))?;

        let scope_context = ScopeCallerContext::prepare(&conn, None)?;
        let resolved = resolve_structural_css_references_on_conn(&conn, &scope_context, None)?;
        assert_eq!(resolved, 1);
        assert_eq!(
            resolved_target(&conn, 100)?,
            Some(vars_file_id),
            "the canonical file id of the resolved path, by identity"
        );
        assert_eq!(
            resolved_target(&conn, 101)?,
            None,
            "an import whose target file is not indexed stays unresolved"
        );
        let certainty: Option<String> =
            conn.query_row("SELECT certainty FROM edge WHERE id = 100", [], |row| {
                row.get(0)
            })?;
        assert_eq!(certainty.as_deref(), Some("certain"));
        Ok(())
    }

    #[test]
    fn cancellation_after_compute_stops_before_apply() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE node (
                id INTEGER PRIMARY KEY,
                kind INTEGER NOT NULL,
                serialized_name TEXT NOT NULL,
                qualified_name TEXT,
                canonical_id TEXT,
                file_node_id INTEGER,
                start_line INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE edge (
                id INTEGER PRIMARY KEY,
                kind INTEGER NOT NULL,
                source_node_id INTEGER NOT NULL,
                target_node_id INTEGER NOT NULL,
                file_node_id INTEGER,
                resolved_source_node_id INTEGER,
                resolved_target_node_id INTEGER,
                confidence REAL,
                certainty TEXT,
                candidate_target_node_ids TEXT,
                callsite_identity TEXT
            );",
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                1_i64,
                NodeKind::FUNCTION as i32,
                "caller",
                "pkg::caller",
                10_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                2_i64,
                NodeKind::FUNCTION as i32,
                "target",
                "pkg::target",
                10_i64
            ],
        )?;
        conn.execute(
            "INSERT INTO edge (id, kind, source_node_id, target_node_id, file_node_id, resolved_target_node_id, confidence, certainty, callsite_identity)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, NULL, NULL)",
            params![100_i64, EdgeKind::CALL as i32, 1_i64, 2_i64, 10_i64],
        )?;

        let pass = ResolutionPass::new();
        let scope_context = ScopeCallerContext::prepare(&conn, None)?;
        let cancel_token = CancellationToken::new();
        let cancel_from_compute = cancel_token.clone();
        let mut semantic_request_stats = SemanticRequestStats::default();
        let mut unresolved_load_ms = 0;
        let mut semantic_candidates_ms = 0;
        let mut compute_ms = 0;
        let mut apply_ms = 0;
        let mut strategy_counters = ResolutionStrategyCounters::default();

        let result = resolve_edges_after_prepare(
            &pass,
            &conn,
            &scope_context,
            &CandidateIndex::default(),
            &SemanticCandidateIndex::default(),
            PreparedResolutionJob {
                edge_kind: EdgeKind::CALL,
                semantic_request_stats: &mut semantic_request_stats,
                unresolved_load_ms: &mut unresolved_load_ms,
                semantic_candidates_ms: &mut semantic_candidates_ms,
                compute_ms: &mut compute_ms,
                apply_ms: &mut apply_ms,
                strategy_counters: &mut strategy_counters,
            },
            move |_pass, _candidate_index, row, _semantic_candidates| {
                cancel_from_compute.cancel();
                Ok(ComputedResolution {
                    update: build_resolved_edge_update(row.0, Some((2_i64, 1.0_f32)), &[])?,
                    strategy: Some(ResolutionStrategy::CallGlobalUnique),
                })
            },
            Some(&cancel_token),
        );

        assert!(result.is_err(), "cancelled resolution should stop");
        assert!(cancel_token.is_cancelled());
        assert_eq!(apply_ms, 0, "apply phase should not run after cancellation");
        let resolved_target: Option<i64> = conn.query_row(
            "SELECT resolved_target_node_id FROM edge WHERE id = ?1",
            params![100_i64],
            |row| row.get(0),
        )?;
        assert_eq!(resolved_target, None);

        Ok(())
    }
}
