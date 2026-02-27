use super::*;

pub(super) fn compute_call_resolution(
    pass: &ResolutionPass,
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
            pass.policy.call_same_file,
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
            pass.policy.call_same_module,
            ResolutionStrategy::CallSameModule,
        ));
    }

    if selected.is_none()
        && !is_common_unqualified
        && let Some(candidate) =
            candidate_index.find_global_unique_readonly(&prepared_name.original, &prepared_name.ascii_lower)
    {
        candidate_ids.push(candidate);
        selected = Some((
            candidate,
            pass.policy.call_global_unique,
            ResolutionStrategy::CallGlobalUnique,
        ));
    }

    if pass.flags.store_candidates && selected.is_none() {
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

pub(super) fn compute_import_resolution(
    pass: &ResolutionPass,
    candidate_index: &CandidateIndex,
    row: &UnresolvedEdgeRow,
    semantic_candidates: &[SemanticResolutionCandidate],
) -> Result<ComputedResolution> {
    let (edge_id, file_id, caller_qualified, target_name, _, _) = row;
    let caller_prefix = caller_qualified.as_deref().and_then(module_prefix);
    let name_candidates = import_name_candidates(target_name, pass.flags.legacy_mode)
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
        if pass.flags.legacy_mode && same_file_selected.is_none() {
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
            && let Some(candidate) =
                candidate_index.find_global_unique_readonly(&name.original, &name.ascii_lower)
        {
            global_stage.push(candidate);
            if !candidate_index.is_same_file_candidate(candidate, *file_id) {
                global_selected = Some(candidate);
            }
        }

        if !pass.flags.legacy_mode
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

    if pass.flags.legacy_mode && same_file_selected.is_some() {
        candidate_ids.extend_stage(&same_file_stage.into_vec(), usize::MAX);
    } else {
        candidate_ids.extend_stage(&same_module_stage.into_vec(), usize::MAX);
        if same_module_selected.is_none() {
            candidate_ids.extend_stage(&global_stage.into_vec(), usize::MAX);
            if global_selected.is_none() && !pass.flags.legacy_mode {
                candidate_ids.extend_stage(&fuzzy_stage.into_vec(), usize::MAX);
            }
        }
    }

    if pass.flags.store_candidates {
        collect_candidate_pool_from_index(candidate_index, &name_candidates, &mut candidate_ids, 8);
    }

    let mut selected: Option<(i64, f32, ResolutionStrategy)> = if let Some(candidate) =
        same_file_selected
    {
        Some((
            candidate,
            pass.policy.import_same_file,
            ResolutionStrategy::ImportSameFile,
        ))
    } else if let Some(candidate) = same_module_selected {
        Some((
            candidate,
            pass.policy.import_same_module,
            ResolutionStrategy::ImportSameModule,
        ))
    } else if let Some(candidate) = global_selected {
        Some((
            candidate,
            pass.policy.import_global_unique,
            ResolutionStrategy::ImportGlobalUnique,
        ))
    } else if let Some(candidate) = fuzzy_selected {
        Some((candidate, pass.policy.import_fuzzy, ResolutionStrategy::ImportFuzzy))
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
