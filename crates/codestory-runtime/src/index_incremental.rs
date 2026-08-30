use crate::index_commit::{
    CoreCommitMode, PreparedCoreCommit, StagedPreparation, next_index_publication,
    rematerialize_staged_proof_resolution_projection, stage_core_publication_identity,
};
use crate::index_coverage::validate_source_policy_exclusions;
use crate::index_timings::{
    IndexingRunSummary, core_indexing_phase_timings, incremental_plan_probe_timings,
};
use crate::search_publication::{
    discard_unpublished_search_generation, read_search_generation_completion,
    search_index_path_for_publication,
};
use crate::search_state_cache::{
    ensure_indexing_active, indexing_cancelled_error, is_indexing_cancelled,
    rebuild_search_state_from_storage_for_runtime, workspace_refresh_inputs,
};
use crate::semantic_projection::{
    ComponentReportRefreshScope, LLM_SYMBOL_DOC_SCHEMA_VERSION, SEMANTIC_POLICY_VERSION,
    SemanticProjectionDocumentSource, SemanticProjectionStats,
    finalize_staged_semantic_docs_for_runtime, semantic_component_key_for_path,
    semantic_file_table_path_map, semantic_graph_dependent_file_ids_by_seed,
};
use crate::workspace_state::runtime_workspace_manifest;
use crate::{
    clamp_u128_to_u32, clamp_usize_to_u32, file_coverage_retryable, runtime_relative_path,
    source_coverage_failure_code, stored_file_coverage_diagnostics,
};
#[cfg(test)]
use crate::{publication::run_incremental_staged_store_hook, test_sidecar_runtime_from_env};
use codestory_contracts::api::{
    ApiError, ApiErrorDetails, AppEventPayload, FileCoverageDiagnosticDto,
    IncrementalPlanProbeOutcomeDto, IndexingPhaseTimings,
};
use codestory_contracts::events::{Event, EventBus};
use codestory_contracts::graph::FileCoverageReason;
use codestory_indexer::{
    CancellationToken, IncrementalIndexingStats, WorkspaceIndexer as V2WorkspaceIndexer,
};
use codestory_store::{
    CURRENT_SCHEMA_VERSION, IndexPublicationMode, IndexPublicationRecord, SnapshotStore,
    SourcePolicyExclusionRecord, StagedSnapshot, StagedSnapshotFinalizeStats, Store,
};
use codestory_workspace::{
    OversizedSourceExclusionCandidate, RefreshExecutionPlan, SourceIndexPolicy,
    WorkspaceInventoryOutcome,
};
use crossbeam_channel::{Receiver, Sender};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;
use uuid::Uuid;

#[cfg(test)]
pub(super) fn index_incremental(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
) -> Result<IndexingRunSummary, ApiError> {
    index_incremental_for_runtime(
        root,
        storage_path,
        events_tx,
        cancel_token,
        &test_sidecar_runtime_from_env(),
        &SourceIndexPolicy::default(),
        &crate::controller_bookmarks::AnnotationsOwned::assume_owned_for_test(),
    )
}

/// Refresh and republish core projections.
///
/// `_annotations_owned` is unused at runtime and load-bearing at compile time:
/// see [`crate::index_full::index_full_for_runtime`].
pub(super) fn index_incremental_for_runtime(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
    _annotations_owned: &crate::controller_bookmarks::AnnotationsOwned,
) -> Result<IndexingRunSummary, ApiError> {
    run_incremental_indexing_common(
        root,
        storage_path,
        events_tx,
        cancel_token,
        runtime,
        source_index_policy,
    )
}

pub(super) fn spawn_progress_forwarder(
    rx: Receiver<Event>,
    progress_tx: Sender<AppEventPayload>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(ev) = rx.recv() {
            match ev {
                Event::IndexingProgress { current, total } => {
                    let _ = progress_tx.send(AppEventPayload::IndexingProgress {
                        current: current.min(u32::MAX as usize) as u32,
                        total: total.min(u32::MAX as usize) as u32,
                    });
                }
                Event::StatusUpdate { message } => {
                    let _ = progress_tx.send(AppEventPayload::StatusUpdate { message });
                }
                _ => {}
            }
        }
    })
}

pub(super) const FULL_REFRESH_REQUIRED_ERROR_CODE: &str = "full_refresh_required";

pub(super) fn full_refresh_required_error(
    root: &Path,
    reason_code: &str,
    reason: impl AsRef<str>,
) -> ApiError {
    let project = root.to_string_lossy().to_string();
    let next_command = format!(
        "codestory-cli index --project {} --refresh full",
        quote_refresh_command_argument(&project)
    );
    ApiError::with_details(
        FULL_REFRESH_REQUIRED_ERROR_CODE,
        format!(
            "Refresh compatibility rejected the request before workspace reads: requested=incremental effective=none required=full reason={}",
            reason.as_ref()
        ),
        ApiErrorDetails {
            cause_code: Some(reason_code.to_string()),
            failed_layer: Some("core_publication_compatibility".to_string()),
            project: Some(project),
            next_commands: vec![next_command.clone()],
            minimum_next: vec![next_command.clone()],
            full_repair: vec![next_command],
            readiness: None,
            embedding_capacity: None,
            embedding_retry: None,
            coverage_gaps: Vec::new(),
        },
    )
}

#[cfg(windows)]
fn quote_refresh_command_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(not(windows))]
fn quote_refresh_command_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn ensure_incremental_refresh_compatible(
    root: &Path,
    storage_path: &Path,
) -> Result<(), ApiError> {
    if !storage_path.is_file() {
        return Err(full_refresh_required_error(
            root,
            "complete_core_publication_missing",
            "complete_core_publication_missing",
        ));
    }
    let schema_version = Store::database_schema_version_observational(storage_path).map_err(
        |error| {
            ApiError::internal(format!(
                "Failed to inspect incremental refresh schema compatibility without recovery: {error}"
            ))
        },
    )?;
    if schema_version < CURRENT_SCHEMA_VERSION {
        let (reason_code, reason) = if schema_version == 0 {
            (
                "complete_core_publication_missing",
                "complete_core_publication_missing".to_string(),
            )
        } else {
            (
                "core_schema_upgrade_required",
                format!(
                    "core_schema_upgrade_required:observed={schema_version}:required={CURRENT_SCHEMA_VERSION}"
                ),
            )
        };
        return Err(full_refresh_required_error(root, reason_code, reason));
    }
    let storage = Store::open_freshness_observational(storage_path).map_err(|error| {
        ApiError::internal(format!(
            "Failed to inspect incremental refresh compatibility: {error}"
        ))
    })?;
    if storage.has_incomplete_incremental_run().map_err(|error| {
        ApiError::internal(format!(
            "Failed to inspect incomplete incremental marker: {error}"
        ))
    })? {
        return Err(full_refresh_required_error(
            root,
            "incomplete_incremental_publication",
            "incomplete_incremental_publication",
        ));
    }
    let Some(publication) = storage.get_complete_index_publication().map_err(|error| {
        ApiError::internal(format!(
            "Failed to inspect complete core publication: {error}"
        ))
    })?
    else {
        return Err(full_refresh_required_error(
            root,
            "complete_core_publication_missing",
            "complete_core_publication_missing",
        ));
    };
    if let Err(error) = storage.validate_structural_text_unit_publication(&publication) {
        return Err(full_refresh_required_error(
            root,
            "structural_publication_incompatible",
            format!("structural_publication_incompatible:{error}"),
        ));
    }
    Ok(())
}

/// Whole-database copies one incremental publication performs.
///
/// `SnapshotStore::clone_live_to_staged` copies the published core into the
/// staged image, promotion copies the previous live image into the rollback
/// backup, and promotion restores the staged image over live. Skipping the
/// staged pipeline avoids all three.
pub(super) const INCREMENTAL_PUBLICATION_DATABASE_COPIES: u32 = 3;

/// Read-only verdict on whether an incremental refresh has any work to do.
///
/// The probe never mutates the published core. Anything it cannot establish
/// resolves to a non-short-circuit outcome so the staged pipeline stays the
/// authority.
pub(super) struct IncrementalPlanProbe {
    pub(super) outcome: IncrementalPlanProbeOutcomeDto,
    pub(super) probe_ms: u32,
    pub(super) files_to_index: u32,
    pub(super) files_to_remove: u32,
    pub(super) live_database_file_bytes: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) publication: Option<IndexPublicationRecord>,
}

impl IncrementalPlanProbe {
    pub(super) fn short_circuited(&self) -> bool {
        self.outcome == IncrementalPlanProbeOutcomeDto::ShortCircuited
    }
}

fn source_policy_exclusions_unchanged(
    stored: &[SourcePolicyExclusionRecord],
    current: &[OversizedSourceExclusionCandidate],
) -> bool {
    if stored.len() != current.len() {
        return false;
    }
    let stored_by_path = stored
        .iter()
        .map(|entry| (entry.normalized_path.as_str(), entry))
        .collect::<HashMap<_, _>>();
    if stored_by_path.len() != stored.len() {
        return false;
    }
    current.iter().all(|candidate| {
        stored_by_path
            .get(candidate.normalized_path.as_str())
            .is_some_and(|previous| {
                previous.content_hash == candidate.content_hash
                    && previous.observed_size == candidate.observed_size
                    && previous.observed_unit_count == candidate.observed_unit_count
                    && previous.policy_version == candidate.policy_version
                    && previous.byte_cap == candidate.byte_cap
                    && previous.structural_unit_cap == candidate.structural_unit_cap
            })
    })
}

/// Decide, from the published core alone, whether the staged pipeline can be skipped.
///
/// Every early return names the reason so the S7 delta-apply decision can see
/// why an unchanged workspace still paid for a republication.
fn evaluate_incremental_plan_probe(
    root: &Path,
    storage_path: &Path,
    source_index_policy: &SourceIndexPolicy,
    probe: &mut IncrementalPlanProbe,
) -> IncrementalPlanProbeOutcomeDto {
    let Ok(storage) = Store::open_freshness_observational(storage_path) else {
        return IncrementalPlanProbeOutcomeDto::ProbeUnavailable;
    };
    let Ok(Some(publication)) = storage.get_complete_index_publication() else {
        return IncrementalPlanProbeOutcomeDto::ProbeUnavailable;
    };
    probe.publication = Some(publication.clone());
    let Ok(workspace) = runtime_workspace_manifest(root, storage_path) else {
        return IncrementalPlanProbeOutcomeDto::ProbeUnavailable;
    };
    let Ok(refresh_inputs) = workspace_refresh_inputs(&storage) else {
        return IncrementalPlanProbeOutcomeDto::ProbeUnavailable;
    };
    let Ok(policy_refresh) =
        workspace.build_execution_outcome_with_policy(&refresh_inputs, source_index_policy)
    else {
        return IncrementalPlanProbeOutcomeDto::ProbeUnavailable;
    };
    probe.files_to_index = clamp_usize_to_u32(policy_refresh.refresh.plan.files_to_index.len());
    probe.files_to_remove = clamp_usize_to_u32(policy_refresh.refresh.plan.files_to_remove.len());
    if policy_refresh.refresh.inventory_outcome != WorkspaceInventoryOutcome::Complete {
        return IncrementalPlanProbeOutcomeDto::InventoryIncomplete;
    }
    if probe.files_to_index != 0 || probe.files_to_remove != 0 {
        return IncrementalPlanProbeOutcomeDto::PlanNotEmpty;
    }
    // A blocking stored coverage gap is adjudicated by
    // `validate_incremental_refresh_coverage` on the staged path. An empty plan
    // does not clear it, so short-circuiting here would keep serving a core the
    // staged pipeline refuses.
    let Ok(stored_coverage) = stored_file_coverage_diagnostics(root, &storage) else {
        return IncrementalPlanProbeOutcomeDto::ProbeUnavailable;
    };
    if stored_coverage
        .iter()
        .any(|entry| entry.reason != FileCoverageReason::ParserPartial)
    {
        return IncrementalPlanProbeOutcomeDto::StoredCoverageGap;
    }
    // Readers validate the published exclusion manifest against the *current*
    // policy identity, not against the exclusion rows alone. A repository with
    // no oversized files has an empty set on both sides, so comparing rows is
    // trivially satisfied and would let a policy change leave the manifest
    // bound to the superseded identity forever.
    if validate_source_policy_exclusions(&storage, root, &publication, source_index_policy).is_err()
    {
        return IncrementalPlanProbeOutcomeDto::SourcePolicyPublicationStale;
    }
    let Ok(stored_exclusions) = storage.get_source_policy_exclusions() else {
        return IncrementalPlanProbeOutcomeDto::ProbeUnavailable;
    };
    if !source_policy_exclusions_unchanged(&stored_exclusions, &policy_refresh.policy_exclusions) {
        return IncrementalPlanProbeOutcomeDto::PolicyExclusionsChanged;
    }
    match storage.get_dense_anchor_publication_manifest() {
        Ok(Some(_)) => {}
        Ok(None) => return IncrementalPlanProbeOutcomeDto::DenseAnchorManifestMissing,
        Err(_) => return IncrementalPlanProbeOutcomeDto::ProbeUnavailable,
    }
    // Use the same strict validation the readiness probe
    // (`complete_core_requires_publication_repair`) applies. Accepting a weaker
    // condition here makes anchor-count, digest, per-row source-identity, and
    // mixed-policy drift unrepairable through the incremental path.
    let dense_anchor_policy_version = match storage.validate_dense_anchor_publication(&publication)
    {
        Ok(manifest) => manifest.policy_version,
        Err(_) => return IncrementalPlanProbeOutcomeDto::DenseAnchorPublicationStale,
    };
    // Mirror `effective_incremental_file_scope`: stored semantic docs built
    // under a previous contract expand an empty scope into a full repair, so
    // that repair must never be skipped.
    if dense_anchor_policy_version != SEMANTIC_POLICY_VERSION {
        return IncrementalPlanProbeOutcomeDto::SemanticDocContractDrift;
    }
    match storage.has_symbol_search_doc_contract_mismatch(
        LLM_SYMBOL_DOC_SCHEMA_VERSION,
        SEMANTIC_POLICY_VERSION,
    ) {
        Ok(false) => {}
        Ok(true) => return IncrementalPlanProbeOutcomeDto::SemanticDocContractDrift,
        Err(_) => return IncrementalPlanProbeOutcomeDto::ProbeUnavailable,
    }
    let Ok(search_path) = search_index_path_for_publication(storage_path, Some(&publication))
    else {
        return IncrementalPlanProbeOutcomeDto::ProbeUnavailable;
    };
    let Ok(generation_id) = Uuid::parse_str(&publication.generation_id) else {
        return IncrementalPlanProbeOutcomeDto::ProbeUnavailable;
    };
    if read_search_generation_completion(&search_path, &generation_id.to_string()).is_none() {
        return IncrementalPlanProbeOutcomeDto::SearchGenerationIncomplete;
    }
    IncrementalPlanProbeOutcomeDto::ShortCircuited
}

pub(super) fn probe_incremental_plan(
    root: &Path,
    storage_path: &Path,
    source_index_policy: &SourceIndexPolicy,
) -> IncrementalPlanProbe {
    let started = Instant::now();
    let live_database_file_bytes = std::fs::metadata(storage_path)
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    let mut probe = IncrementalPlanProbe {
        outcome: IncrementalPlanProbeOutcomeDto::ProbeUnavailable,
        probe_ms: 0,
        files_to_index: 0,
        files_to_remove: 0,
        live_database_file_bytes,
        publication: None,
    };
    probe.outcome =
        evaluate_incremental_plan_probe(root, storage_path, source_index_policy, &mut probe);
    probe.probe_ms = clamp_u128_to_u32(started.elapsed().as_millis());
    probe
}

fn incremental_execution_plan(
    staged: &mut StagedSnapshot,
    root: &Path,
    storage_path: &Path,
    source_index_policy: &SourceIndexPolicy,
) -> Result<(RefreshExecutionPlan, Vec<OversizedSourceExclusionCandidate>), ApiError> {
    let workspace = runtime_workspace_manifest(root, storage_path)
        .map_err(|error| ApiError::internal(format!("Failed to open project: {error}")))?;
    let refresh_inputs = workspace_refresh_inputs(staged.store_mut())?;
    let policy_refresh = workspace
        .build_execution_outcome_with_policy(&refresh_inputs, source_index_policy)
        .map_err(|error| ApiError::internal(format!("Failed to generate refresh info: {error}")))?;
    if policy_refresh.refresh.inventory_outcome == WorkspaceInventoryOutcome::Complete {
        return Ok((
            policy_refresh.refresh.plan,
            policy_refresh.policy_exclusions,
        ));
    }
    let reason =
        if policy_refresh.refresh.inventory_outcome == WorkspaceInventoryOutcome::Unreadable {
            FileCoverageReason::Unreadable
        } else {
            FileCoverageReason::DiscoveryIncomplete
        };
    let mut gaps = policy_refresh
        .refresh
        .inventory_issues
        .iter()
        .map(|issue| FileCoverageDiagnosticDto {
            path: runtime_relative_path(root, &issue.path),
            reason,
            retryable: file_coverage_retryable(reason),
            verified_source: false,
            projection_available: false,
        })
        .collect::<Vec<_>>();
    if gaps.is_empty() {
        gaps.push(FileCoverageDiagnosticDto {
            path: ".".into(),
            reason,
            retryable: file_coverage_retryable(reason),
            verified_source: false,
            projection_available: false,
        });
    }
    Err(ApiError::source_coverage_failure(
        source_coverage_failure_code(&gaps),
        format!(
            "Incremental refresh requires a complete source inventory; discovery was {:?}.",
            policy_refresh.refresh.inventory_outcome
        ),
        gaps,
    ))
}

struct IncrementalSemanticPlan {
    previous_indexed_file_ids_by_path: HashMap<String, codestory_contracts::graph::NodeId>,
    policy_excluded_seed_file_ids: HashSet<codestory_contracts::graph::NodeId>,
    previous_dependents_by_seed:
        HashMap<codestory_contracts::graph::NodeId, HashSet<codestory_contracts::graph::NodeId>>,
    component_reports: ComponentReportRefreshScope,
}

fn plan_incremental_semantics(
    staged: &mut StagedSnapshot,
    root: &Path,
    execution_plan: &RefreshExecutionPlan,
) -> Result<IncrementalSemanticPlan, ApiError> {
    let mut planned_seed_file_ids = execution_plan
        .files_to_remove
        .iter()
        .copied()
        .map(codestory_contracts::graph::NodeId)
        .collect::<HashSet<_>>();
    let mut previous_indexed_file_ids_by_path = HashMap::new();
    for path in &execution_plan.files_to_index {
        let normalized_path = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        if let Some(file_info) = staged
            .store_mut()
            .get_file_by_path(&normalized_path)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to resolve previous semantic scope for {}: {error}",
                    normalized_path.display()
                ))
            })?
        {
            let file_id = codestory_contracts::graph::NodeId(file_info.id);
            planned_seed_file_ids.insert(file_id);
            previous_indexed_file_ids_by_path
                .insert(runtime_relative_path(root, &normalized_path), file_id);
        }
    }
    let previous_dependents_by_seed =
        semantic_graph_dependent_file_ids_by_seed(staged.store_mut(), &planned_seed_file_ids)?;
    let existing_file_paths = semantic_file_table_path_map(
        staged
            .store_mut()
            .get_files()
            .map_err(|error| ApiError::internal(format!("Failed to load files: {error}")))?,
    );
    let mut removed_component_keys = HashSet::new();
    for file_id in &execution_plan.files_to_remove {
        let path = existing_file_paths
            .get(&codestory_contracts::graph::NodeId(*file_id))
            .ok_or_else(|| {
                ApiError::internal(format!(
                    "Removed file is missing from staged component scope: {file_id}"
                ))
            })?;
        if let Some(component_key) = semantic_component_key_for_path(Some(path)) {
            removed_component_keys.insert(component_key);
        }
    }
    Ok(IncrementalSemanticPlan {
        previous_indexed_file_ids_by_path,
        policy_excluded_seed_file_ids: HashSet::new(),
        previous_dependents_by_seed,
        component_reports: ComponentReportRefreshScope {
            previous_file_paths: existing_file_paths,
            removed_component_keys,
        },
    })
}

struct IncrementalIndexerContext<'a> {
    root: &'a Path,
    events_tx: &'a Sender<AppEventPayload>,
    cancel_token: Option<&'a CancellationToken>,
    source_index_policy: &'a SourceIndexPolicy,
    execution_plan: &'a RefreshExecutionPlan,
}

fn run_incremental_indexer(
    staged: &mut StagedSnapshot,
    context: IncrementalIndexerContext<'_>,
    semantic_plan: &mut IncrementalSemanticPlan,
    policy_exclusions: &mut Vec<OversizedSourceExclusionCandidate>,
) -> Result<IncrementalIndexingStats, ApiError> {
    let IncrementalIndexerContext {
        root,
        events_tx,
        cancel_token,
        source_index_policy,
        execution_plan,
    } = context;
    let total_files = execution_plan.files_to_index.len().min(u32::MAX as usize) as u32;
    let _ = events_tx.send(AppEventPayload::IndexingStarted {
        file_count: total_files,
    });
    #[cfg(test)]
    run_incremental_staged_store_hook(staged.store_mut());
    let bus = EventBus::new();
    let forwarder = spawn_progress_forwarder(bus.receiver(), events_tx.clone());
    if let Err(error) = ensure_indexing_active(cancel_token) {
        drop(bus);
        let _ = forwarder.join();
        return Err(error);
    }
    let result = V2WorkspaceIndexer::new(root.to_path_buf())
        .with_source_index_policy(source_index_policy.clone())
        .run_with_policy_exclusions(staged.store_mut(), execution_plan, &bus, cancel_token);
    drop(bus);
    let _ = forwarder.join();
    let outcome = match result {
        Ok(_) if is_indexing_cancelled(cancel_token) => return Err(indexing_cancelled_error()),
        Ok(outcome) => outcome,
        Err(_) if is_indexing_cancelled(cancel_token) => return Err(indexing_cancelled_error()),
        Err(error) => return Err(ApiError::internal(format!("Indexing failed: {error}"))),
    };
    for exclusion in &outcome.policy_exclusions {
        if let Some(file_id) = semantic_plan
            .previous_indexed_file_ids_by_path
            .get(&exclusion.normalized_path)
        {
            semantic_plan.policy_excluded_seed_file_ids.insert(*file_id);
            if let Some(component_key) = semantic_plan
                .component_reports
                .previous_file_paths
                .get(file_id)
                .and_then(|path| semantic_component_key_for_path(Some(path)))
            {
                semantic_plan
                    .component_reports
                    .removed_component_keys
                    .insert(component_key);
            }
        }
    }
    policy_exclusions.extend(outcome.policy_exclusions);
    Ok(outcome.stats)
}

fn validate_incremental_refresh_coverage(
    staged: &mut StagedSnapshot,
    root: &Path,
) -> Result<(), ApiError> {
    let blocking_gaps = stored_file_coverage_diagnostics(root, staged.store_mut())?
        .into_iter()
        .filter(|entry| entry.reason != FileCoverageReason::ParserPartial)
        .collect::<Vec<_>>();
    if blocking_gaps.is_empty() {
        return Ok(());
    }
    let count = blocking_gaps.len();
    let sample = blocking_gaps
        .iter()
        .take(3)
        .map(|entry| format!("{} ({})", entry.path, entry.reason.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    Err(ApiError::source_coverage_failure(
        source_coverage_failure_code(&blocking_gaps),
        format!(
            "Incremental refresh could not verify {count} scheduled file(s): {sample}. The previous complete publication was preserved."
        ),
        blocking_gaps,
    ))
}

fn incremental_semantic_refresh_scope(
    staged: &mut StagedSnapshot,
    root: &Path,
    execution_plan: &RefreshExecutionPlan,
    semantic_plan: &IncrementalSemanticPlan,
) -> Result<HashSet<codestory_contracts::graph::NodeId>, ApiError> {
    let mut refresh_seed_file_ids = HashSet::new();
    for path in &execution_plan.files_to_index {
        let normalized_path = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        let file_info = staged
            .store_mut()
            .get_file_by_path(&normalized_path)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to resolve indexed semantic scope for {}: {error}",
                    normalized_path.display()
                ))
            })?;
        if let Some(file_info) = file_info
            && file_info.complete
        {
            refresh_seed_file_ids.insert(codestory_contracts::graph::NodeId(file_info.id));
        }
    }
    refresh_seed_file_ids.extend(
        execution_plan
            .files_to_remove
            .iter()
            .copied()
            .map(codestory_contracts::graph::NodeId),
    );
    refresh_seed_file_ids.extend(&semantic_plan.policy_excluded_seed_file_ids);
    let current_dependents_by_seed =
        semantic_graph_dependent_file_ids_by_seed(staged.store_mut(), &refresh_seed_file_ids)?;
    let mut refresh_scope = refresh_seed_file_ids.clone();
    for seed_file_id in &refresh_seed_file_ids {
        if let Some(file_ids) = semantic_plan.previous_dependents_by_seed.get(seed_file_id) {
            refresh_scope.extend(file_ids);
        }
        if let Some(file_ids) = current_dependents_by_seed.get(seed_file_id) {
            refresh_scope.extend(file_ids);
        }
    }
    Ok(refresh_scope)
}

struct PreparedIncrementalRefresh {
    staged: StagedSnapshot,
    publication: IndexPublicationRecord,
    stats: IncrementalIndexingStats,
    finalize_stats: StagedSnapshotFinalizeStats,
    detail_snapshot_ms: u32,
    semantic_stats: SemanticProjectionStats,
    semantic_refresh_scope: HashSet<codestory_contracts::graph::NodeId>,
    policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
    probe: IncrementalPlanProbe,
}

/// Either the staged republication is required, or the published core already
/// satisfies the request and nothing may be written.
enum IncrementalRefreshPreparation {
    Unchanged(IncrementalPlanProbe),
    Prepared(Box<PreparedIncrementalRefresh>),
}

fn prepare_incremental_refresh(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
) -> Result<IncrementalRefreshPreparation, ApiError> {
    ensure_incremental_refresh_compatible(root, storage_path)?;
    ensure_indexing_active(cancel_token)?;
    let probe = probe_incremental_plan(root, storage_path, source_index_policy);
    // Cancellation raised during the probe still cancels the request, so a
    // short-circuit never reports success for an abandoned refresh.
    ensure_indexing_active(cancel_token)?;
    if probe.short_circuited() {
        return Ok(IncrementalRefreshPreparation::Unchanged(probe));
    }
    let staged = SnapshotStore::clone_live_to_staged(storage_path).map_err(|error| {
        ApiError::internal(format!(
            "Failed to clone live storage for incremental build: {error}"
        ))
    })?;
    let mut preparation = StagedPreparation::new(staged);
    let previous_publication = preparation
        .staged_mut()
        .store_mut()
        .get_index_publication()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to read staged publication identity: {error}"
            ))
        })?;
    let rebuild_complete_dense_anchor_set = preparation
        .staged_mut()
        .store_mut()
        .get_dense_anchor_publication_manifest()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to read staged dense anchor publication identity: {error}"
            ))
        })?
        .is_none();
    let publication = next_index_publication(
        previous_publication.as_ref(),
        IndexPublicationMode::Incremental,
        &Uuid::new_v4().to_string(),
    )?;
    let source_identity = format!("core:{}:{}", publication.generation_id, publication.run_id);
    preparation
        .staged_mut()
        .store_mut()
        .begin_incremental_run()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to persist staged incomplete index marker: {error}"
            ))
        })?;
    preparation
        .staged_mut()
        .store_mut()
        .invalidate_grounding_snapshots()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to invalidate staged derived index snapshots: {error}"
            ))
        })?;
    let (execution_plan, mut policy_exclusions) = incremental_execution_plan(
        preparation.staged_mut(),
        root,
        storage_path,
        source_index_policy,
    )?;
    let mut semantic_plan =
        plan_incremental_semantics(preparation.staged_mut(), root, &execution_plan)?;
    let stats = run_incremental_indexer(
        preparation.staged_mut(),
        IncrementalIndexerContext {
            root,
            events_tx,
            cancel_token,
            source_index_policy,
            execution_plan: &execution_plan,
        },
        &mut semantic_plan,
        &mut policy_exclusions,
    )?;
    validate_incremental_refresh_coverage(preparation.staged_mut(), root)?;
    rematerialize_staged_proof_resolution_projection(
        preparation.staged_mut(),
        &publication,
        cancel_token,
    )?;
    let semantic_refresh_scope = incremental_semantic_refresh_scope(
        preparation.staged_mut(),
        root,
        &execution_plan,
        &semantic_plan,
    )?;
    let semantic_stats = finalize_staged_semantic_docs_for_runtime(
        preparation.staged_mut().store_mut(),
        (!rebuild_complete_dense_anchor_set).then_some(&semantic_refresh_scope),
        (!rebuild_complete_dense_anchor_set).then_some(&semantic_plan.component_reports),
        &source_identity,
        cancel_token,
        runtime,
        SemanticProjectionDocumentSource::SourceFiles {
            max_file_bytes: source_index_policy.byte_cap,
        },
    )?;
    ensure_indexing_active(cancel_token)?;
    let finalize_stats = preparation
        .staged_mut()
        .snapshots()
        .finalize_staged()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to finalize staged incremental storage: {error}"
            ))
        })?;
    let detail_started = Instant::now();
    preparation
        .staged_mut()
        .snapshots()
        .refresh_detail()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to refresh staged grounding detail snapshot: {error}"
            ))
        })?;
    ensure_indexing_active(cancel_token)?;
    Ok(IncrementalRefreshPreparation::Prepared(Box::new(
        PreparedIncrementalRefresh {
            staged: preparation.release(),
            publication,
            stats,
            finalize_stats,
            detail_snapshot_ms: clamp_u128_to_u32(detail_started.elapsed().as_millis()),
            semantic_stats,
            semantic_refresh_scope,
            policy_exclusions,
            probe,
        },
    )))
}

fn run_incremental_indexing_common(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
) -> Result<IndexingRunSummary, ApiError> {
    let prepared = prepare_incremental_refresh(
        root,
        storage_path,
        events_tx,
        cancel_token,
        runtime,
        source_index_policy,
    )?;
    let prepared = match prepared {
        IncrementalRefreshPreparation::Unchanged(probe) => {
            return Ok(unchanged_incremental_run_summary(probe));
        }
        IncrementalRefreshPreparation::Prepared(prepared) => prepared,
    };
    let PreparedIncrementalRefresh {
        mut staged,
        publication,
        stats: index_stats,
        finalize_stats: staged_finalize_stats,
        detail_snapshot_ms,
        semantic_stats: staged_semantic_stats,
        semantic_refresh_scope: llm_refresh_scope,
        policy_exclusions,
        probe,
    } = *prepared;
    let workspace = match runtime_workspace_manifest(root, storage_path) {
        Ok(workspace) => workspace,
        Err(error) => {
            let _ = staged.discard();
            return Err(ApiError::internal(format!(
                "Failed to reopen project: {error}"
            )));
        }
    };
    if let Err(error) = stage_core_publication_identity(
        &mut staged,
        root,
        &workspace,
        &publication,
        &policy_exclusions,
        source_index_policy,
        cancel_token,
    ) {
        let _ = staged.discard();
        return Err(error);
    }
    let prepared_search_state = match rebuild_search_state_from_storage_for_runtime(
        staged.store_mut(),
        storage_path,
        Some(&llm_refresh_scope),
        false,
        runtime,
        cancel_token,
        None,
    ) {
        Ok(state) => state,
        Err(error) => {
            let _ = staged.discard();
            discard_unpublished_search_generation(storage_path, &publication);
            return Err(error);
        }
    };
    if is_indexing_cancelled(cancel_token) {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(indexing_cancelled_error());
    }
    let prepared_commit =
        PreparedCoreCommit::new(staged, prepared_search_state, storage_path, &publication);
    let (prepared_search_state, staged_publish_stats, publish_duration) =
        prepared_commit.commit(CoreCommitMode::Incremental, cancel_token)?;
    let mut phase_timings = core_indexing_phase_timings(
        &index_stats,
        staged_finalize_stats,
        detail_snapshot_ms,
        staged_publish_stats,
        publish_duration,
        staged_semantic_stats.semantic_context_index_ms,
    );
    phase_timings.incremental_plan_probe = Some(incremental_plan_probe_timings(&probe));
    Ok(IndexingRunSummary {
        phase_timings,
        staged_semantic_stats,
        llm_refresh_scope: Some(llm_refresh_scope),
        #[cfg(test)]
        publication,
        prepared_search_state: Some(prepared_search_state),
        unchanged_publication: false,
    })
}

/// Summarize a refresh that proved the published core already satisfied the
/// request. No staged image was opened, so no publication or search generation
/// was written and the previous ones stay pinned.
fn unchanged_incremental_run_summary(probe: IncrementalPlanProbe) -> IndexingRunSummary {
    let phase_timings = IndexingPhaseTimings {
        incremental_plan_probe: Some(incremental_plan_probe_timings(&probe)),
        ..IndexingPhaseTimings::default()
    };
    IndexingRunSummary {
        phase_timings,
        staged_semantic_stats: SemanticProjectionStats::default(),
        llm_refresh_scope: None,
        #[cfg(test)]
        publication: probe
            .publication
            .clone()
            .expect("a short-circuited refresh observed the complete core publication"),
        prepared_search_state: None,
        unchanged_publication: true,
    }
}
