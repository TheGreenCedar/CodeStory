use crate::index_commit::{
    CoreCommitMode, PreparedCoreCommit, StagedPreparation, next_index_publication,
    rematerialize_staged_proof_resolution_projection, stage_core_publication_identity,
};
use crate::index_coverage::validate_source_policy_exclusions;
use crate::index_timings::{
    IndexingRunSummary, core_indexing_phase_timings, incremental_plan_probe_timings,
};
use crate::search_publication::{
    discard_unpublished_search_generation, materialize_equivalent_search_generation,
    read_search_generation_completion, search_index_path_for_publication,
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
    IncrementalCoreWallTimings, IncrementalPlanProbeOutcomeDto, IncrementalScheduledPathActionDto,
    IncrementalScheduledPathDto, IncrementalScheduledPathReasonDto, IndexingPhaseTimings,
};
use codestory_contracts::events::{Event, EventBus};
use codestory_contracts::graph::FileCoverageReason;
use codestory_contracts::validation_receipts::ArtifactSeal;
use codestory_indexer::{
    CancellationToken, IncrementalIndexingStats, WorkspaceIndexer as V2WorkspaceIndexer,
};
use codestory_store::{
    CURRENT_SCHEMA_VERSION, IndexPublicationMode, IndexPublicationRecord, SnapshotStore,
    SourcePolicyExclusionRecord, StagedSnapshot, StagedSnapshotFinalizeStats,
    StagedSnapshotPublishStats, Store,
};
use codestory_workspace::{
    OversizedSourceExclusionCandidate, RefreshExecutionPlan, SourceIndexPolicy,
    WorkspaceInventoryOutcome,
};
use crossbeam_channel::{Receiver, Sender};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
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
    index_incremental_for_runtime_with_probe(
        root,
        storage_path,
        events_tx,
        cancel_token,
        runtime,
        source_index_policy,
        _annotations_owned,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn index_incremental_for_runtime_with_probe(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
    _annotations_owned: &crate::controller_bookmarks::AnnotationsOwned,
    precomputed_probe: Option<IncrementalPlanProbe>,
) -> Result<IndexingRunSummary, ApiError> {
    run_incremental_indexing_common(
        root,
        storage_path,
        events_tx,
        cancel_token,
        runtime,
        source_index_policy,
        precomputed_probe,
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
    if !codestory_store::core_database_exists(storage_path).map_err(|error| {
        ApiError::internal(format!(
            "Failed to resolve incremental core publication: {error}"
        ))
    })? {
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
pub(super) const INCREMENTAL_PUBLICATION_DATABASE_COPIES: u32 = 0;

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
    execution_plan: Option<RefreshExecutionPlan>,
    policy_exclusions: Option<Vec<OversizedSourceExclusionCandidate>>,
    source_seals: Option<Vec<ArtifactSeal>>,
    scheduled_paths: Vec<IncrementalScheduledPathDto>,
}

impl IncrementalPlanProbe {
    pub(super) fn short_circuited(&self) -> bool {
        self.outcome == IncrementalPlanProbeOutcomeDto::ShortCircuited
    }

    /// The probe completed the unbounded source inventory and sealed every
    /// admitted source it planned against. An activation may carry this fact
    /// forward only while the same filesystem observer epoch remains stable.
    pub(super) fn has_complete_source_inventory(&self) -> bool {
        self.execution_plan.is_some()
            && self.policy_exclusions.is_some()
            && self.source_seals.is_some()
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
    probe.source_seals = ArtifactSeal::observe_all(&policy_refresh.inventory_files).ok();
    probe.scheduled_paths =
        incremental_scheduled_paths(root, &refresh_inputs, &policy_refresh.refresh.plan);
    probe.execution_plan = Some(policy_refresh.refresh.plan.clone());
    probe.policy_exclusions = Some(policy_refresh.policy_exclusions.clone());
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
    let live_database_file_bytes = codestory_store::resolve_core_database_path(storage_path)
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    let mut probe = IncrementalPlanProbe {
        outcome: IncrementalPlanProbeOutcomeDto::ProbeUnavailable,
        probe_ms: 0,
        files_to_index: 0,
        files_to_remove: 0,
        live_database_file_bytes,
        publication: None,
        execution_plan: None,
        policy_exclusions: None,
        source_seals: None,
        scheduled_paths: Vec::new(),
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
) -> Result<
    (
        RefreshExecutionPlan,
        Vec<OversizedSourceExclusionCandidate>,
        Vec<IncrementalScheduledPathDto>,
        Vec<PathBuf>,
    ),
    ApiError,
> {
    let workspace = runtime_workspace_manifest(root, storage_path)
        .map_err(|error| ApiError::internal(format!("Failed to open project: {error}")))?;
    let refresh_inputs = workspace_refresh_inputs(staged.store_mut())?;
    let policy_refresh = workspace
        .build_execution_outcome_with_policy(&refresh_inputs, source_index_policy)
        .map_err(|error| ApiError::internal(format!("Failed to generate refresh info: {error}")))?;
    if policy_refresh.refresh.inventory_outcome == WorkspaceInventoryOutcome::Complete {
        let scheduled_paths =
            incremental_scheduled_paths(root, &refresh_inputs, &policy_refresh.refresh.plan);
        return Ok((
            policy_refresh.refresh.plan,
            policy_refresh.policy_exclusions,
            scheduled_paths,
            policy_refresh.inventory_files,
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

fn incremental_scheduled_paths(
    root: &Path,
    refresh_inputs: &codestory_contracts::workspace::RefreshInputs,
    execution_plan: &RefreshExecutionPlan,
) -> Vec<IncrementalScheduledPathDto> {
    let stored = refresh_inputs.inventory_map();
    let stored_by_path = stored
        .values()
        .map(|state| (runtime_relative_path(root, &state.path), state))
        .collect::<HashMap<_, _>>();
    let stored_by_id = stored
        .values()
        .map(|state| (state.id, state))
        .collect::<HashMap<_, _>>();
    let mut scheduled = execution_plan
        .files_to_index
        .iter()
        .map(|path| {
            let path = runtime_relative_path(root, path);
            let reason = match stored_by_path.get(&path) {
                None => IncrementalScheduledPathReasonDto::NewFile,
                Some(state) if state.retry_required => {
                    IncrementalScheduledPathReasonDto::RetryRequired
                }
                Some(state) if !state.indexed => IncrementalScheduledPathReasonDto::NotIndexed,
                // Completeness alone does not schedule a refresh. If an indexed,
                // non-retryable partial file reached this plan, its verified
                // source identity changed.
                Some(state) if !state.complete => {
                    IncrementalScheduledPathReasonDto::SourceIdentityChanged
                }
                Some(_) => IncrementalScheduledPathReasonDto::SourceIdentityChanged,
            };
            IncrementalScheduledPathDto {
                path,
                action: IncrementalScheduledPathActionDto::Index,
                reason,
            }
        })
        .collect::<Vec<_>>();
    scheduled.extend(execution_plan.files_to_remove.iter().map(|file_id| {
        IncrementalScheduledPathDto {
            path: stored_by_id
                .get(file_id)
                .map(|state| runtime_relative_path(root, &state.path))
                .unwrap_or_else(|| format!("file-id:{file_id}")),
            action: IncrementalScheduledPathActionDto::Remove,
            reason: IncrementalScheduledPathReasonDto::VerifiedAbsent,
        }
    }));
    scheduled.sort_by(|left, right| {
        let action_rank = |action| match action {
            IncrementalScheduledPathActionDto::Index => 0_u8,
            IncrementalScheduledPathActionDto::Remove => 1_u8,
        };
        left.path
            .cmp(&right.path)
            .then_with(|| action_rank(left.action).cmp(&action_rank(right.action)))
    });
    scheduled
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
    previous_publication: Option<IndexPublicationRecord>,
    stats: IncrementalIndexingStats,
    finalize_stats: StagedSnapshotFinalizeStats,
    detail_snapshot_ms: u32,
    semantic_stats: SemanticProjectionStats,
    reused_dense_anchor_projection: bool,
    source_identity_file_ids: Option<Vec<i64>>,
    retrieval_refresh_receipt: Option<codestory_retrieval::IncrementalRetrievalRefreshReceipt>,
    semantic_refresh_scope: HashSet<codestory_contracts::graph::NodeId>,
    policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
    probe: IncrementalPlanProbe,
    wall: IncrementalCoreWallDurations,
    derived_timings: IncrementalDerivedStageTimings,
}

#[derive(Debug, Default)]
struct IncrementalDerivedStageTimings {
    coverage_validation_ms: u32,
    proof_projection_ms: u32,
    semantic_scope_ms: u32,
    semantic_projection_ms: u32,
    grounding_snapshot_ms: u32,
    publication_identity_ms: u32,
    search_generation_ms: u32,
}

#[derive(Debug, Default)]
struct IncrementalCoreWallDurations {
    discovery_and_scheduling: Duration,
    stage_open: Duration,
    parse_and_extraction: Duration,
    core_staging_and_mutation: Duration,
    candidate_sealing: Duration,
    scheduled_paths: Vec<IncrementalScheduledPathDto>,
}

impl IncrementalCoreWallDurations {
    fn finish(
        mut self,
        core_refresh: Duration,
        commit: Option<(Duration, &StagedSnapshotPublishStats)>,
    ) -> IncrementalCoreWallTimings {
        let mut pointer_publication = Duration::ZERO;
        let mut lock_wait = Duration::ZERO;
        let mut process_and_ipc = Duration::ZERO;
        if let Some((commit_wall, publish_stats)) = commit {
            let seal_ms = publish_stats
                .sqlite_checkpoint_ms
                .unwrap_or_default()
                .saturating_add(publish_stats.sqlite_sync_ms.unwrap_or_default())
                .saturating_add(publish_stats.core_promotion.candidate_validation_ms)
                .saturating_add(publish_stats.core_promotion.generation_install_ms);
            self.candidate_sealing = self
                .candidate_sealing
                .saturating_add(Duration::from_millis(u64::from(seal_ms)));
            lock_wait = Duration::from_millis(u64::from(publish_stats.core_promotion.lock_wait_ms));
            pointer_publication = Duration::from_millis(u64::from(
                publish_stats.core_promotion.pointer_publication_ms,
            ));
            process_and_ipc = commit_wall
                .saturating_sub(Duration::from_millis(u64::from(seal_ms)))
                .saturating_sub(lock_wait)
                .saturating_sub(pointer_publication);
        }
        let named = self
            .discovery_and_scheduling
            .saturating_add(self.stage_open)
            .saturating_add(self.parse_and_extraction)
            .saturating_add(self.core_staging_and_mutation)
            .saturating_add(self.candidate_sealing)
            .saturating_add(pointer_publication)
            .saturating_add(lock_wait)
            .saturating_add(process_and_ipc);
        let mut receipt = IncrementalCoreWallTimings {
            core_refresh_ms: clamp_u128_to_u32(core_refresh.as_millis()),
            discovery_and_scheduling_ms: clamp_u128_to_u32(
                self.discovery_and_scheduling.as_millis(),
            ),
            stage_open_ms: clamp_u128_to_u32(self.stage_open.as_millis()),
            parse_and_extraction_ms: clamp_u128_to_u32(self.parse_and_extraction.as_millis()),
            core_staging_and_mutation_ms: clamp_u128_to_u32(
                self.core_staging_and_mutation.as_millis(),
            ),
            candidate_sealing_ms: clamp_u128_to_u32(self.candidate_sealing.as_millis()),
            pointer_publication_ms: clamp_u128_to_u32(pointer_publication.as_millis()),
            lock_wait_ms: clamp_u128_to_u32(lock_wait.as_millis()),
            process_and_ipc_ms: clamp_u128_to_u32(process_and_ipc.as_millis()),
            unattributed_ms: clamp_u128_to_u32(core_refresh.saturating_sub(named).as_millis()),
            scheduled_paths: self.scheduled_paths,
        };
        let named_ms = receipt
            .discovery_and_scheduling_ms
            .saturating_add(receipt.stage_open_ms)
            .saturating_add(receipt.parse_and_extraction_ms)
            .saturating_add(receipt.core_staging_and_mutation_ms)
            .saturating_add(receipt.candidate_sealing_ms)
            .saturating_add(receipt.pointer_publication_ms)
            .saturating_add(receipt.lock_wait_ms)
            .saturating_add(receipt.process_and_ipc_ms);
        receipt.unattributed_ms = receipt.core_refresh_ms.saturating_sub(named_ms);
        receipt
    }
}

/// Either the staged republication is required, or the published core already
/// satisfies the request and nothing may be written.
enum IncrementalRefreshPreparation {
    Unchanged {
        probe: IncrementalPlanProbe,
        wall: IncrementalCoreWallDurations,
    },
    Prepared(Box<PreparedIncrementalRefresh>),
}

fn prepare_incremental_refresh(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
    precomputed_probe: Option<IncrementalPlanProbe>,
) -> Result<IncrementalRefreshPreparation, ApiError> {
    let mut wall = IncrementalCoreWallDurations::default();
    let mut derived_timings = IncrementalDerivedStageTimings::default();
    let discovery_started = Instant::now();
    ensure_incremental_refresh_compatible(root, storage_path)?;
    ensure_indexing_active(cancel_token)?;
    let mut probe = precomputed_probe
        .unwrap_or_else(|| probe_incremental_plan(root, storage_path, source_index_policy));
    // Cancellation raised during the probe still cancels the request, so a
    // short-circuit never reports success for an abandoned refresh.
    ensure_indexing_active(cancel_token)?;
    wall.discovery_and_scheduling = discovery_started.elapsed();
    if probe.short_circuited() {
        return Ok(IncrementalRefreshPreparation::Unchanged { probe, wall });
    }
    let stage_open_started = Instant::now();
    let staged = SnapshotStore::clone_live_to_staged(storage_path).map_err(|error| {
        ApiError::internal(format!(
            "Failed to clone live storage for incremental build: {error}"
        ))
    })?;
    wall.stage_open = stage_open_started.elapsed();
    let staging_started = Instant::now();
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
    let inherited_grounding_snapshots_ready = preparation
        .staged_mut()
        .snapshots()
        .has_ready_summary()
        .and_then(|summary| {
            preparation
                .staged_mut()
                .snapshots()
                .has_ready_detail()
                .map(|detail| summary && detail)
        })
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to inspect inherited grounding snapshots: {error}"
            ))
        })?;
    preparation
        .staged_mut()
        .store_mut()
        .begin_incremental_run()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to persist staged incomplete index marker: {error}"
            ))
        })?;
    wall.core_staging_and_mutation = staging_started.elapsed();
    let retained_plan_matches_stage = probe.publication.as_ref() == previous_publication.as_ref()
        && probe.execution_plan.is_some()
        && probe.policy_exclusions.is_some()
        && probe.source_seals.is_some();
    let (execution_plan, mut policy_exclusions, source_seals, scheduled_paths) =
        if retained_plan_matches_stage {
            (
                probe
                    .execution_plan
                    .take()
                    .expect("retained plan was checked above"),
                probe
                    .policy_exclusions
                    .take()
                    .expect("retained exclusions were checked above"),
                probe
                    .source_seals
                    .take()
                    .expect("retained source seals were checked above"),
                std::mem::take(&mut probe.scheduled_paths),
            )
        } else {
            let discovery_started = Instant::now();
            let plan = incremental_execution_plan(
                preparation.staged_mut(),
                root,
                storage_path,
                source_index_policy,
            )?;
            wall.discovery_and_scheduling = wall
                .discovery_and_scheduling
                .saturating_add(discovery_started.elapsed());
            let source_seals = ArtifactSeal::observe_all(&plan.3).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to seal complete incremental source inventory: {error}"
                ))
            })?;
            (plan.0, plan.1, source_seals, plan.2)
        };
    wall.scheduled_paths = scheduled_paths;
    let staging_started = Instant::now();
    let mut semantic_plan =
        plan_incremental_semantics(preparation.staged_mut(), root, &execution_plan)?;
    wall.core_staging_and_mutation = wall
        .core_staging_and_mutation
        .saturating_add(staging_started.elapsed());
    let parse_started = Instant::now();
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
    wall.parse_and_extraction = parse_started.elapsed();
    let staging_started = Instant::now();
    let coverage_started = Instant::now();
    validate_incremental_refresh_coverage(preparation.staged_mut(), root)?;
    derived_timings.coverage_validation_ms =
        clamp_u128_to_u32(coverage_started.elapsed().as_millis());
    let source_identity_file_ids = (!stats.graph_projection_changed)
        .then(|| {
            execution_plan
                .files_to_index
                .iter()
                .filter_map(|path| execution_plan.existing_file_ids.get(path).copied())
                .collect::<Vec<_>>()
        })
        .filter(|file_ids| file_ids.len() == execution_plan.files_to_index.len());
    let retrieval_refresh_receipt = previous_publication.as_ref().and_then(|previous| {
        if stats.graph_projection_changed
            || !execution_plan.files_to_remove.is_empty()
            || execution_plan.files_to_index.is_empty()
            || execution_plan
                .files_to_index
                .iter()
                .any(|path| !execution_plan.existing_file_ids.contains_key(path))
        {
            return None;
        }
        let mut changed_existing_sources = execution_plan
            .files_to_index
            .iter()
            .map(|path| runtime_relative_path(root, path))
            .collect::<Vec<_>>();
        changed_existing_sources.sort();
        changed_existing_sources.dedup();
        (changed_existing_sources.len() == execution_plan.files_to_index.len()).then(|| {
            codestory_retrieval::IncrementalRetrievalRefreshReceipt {
                project_root: root.to_path_buf(),
                storage_path: storage_path.to_path_buf(),
                previous_core: previous.clone(),
                current_core: publication.clone(),
                changed_existing_sources,
                source_seals: source_seals.clone(),
                source_policy: source_index_policy.clone(),
                graph_projection_changed: false,
            }
        })
    });
    let proof_started = Instant::now();
    let proof_rebound = match (
        previous_publication.as_ref(),
        source_identity_file_ids.as_deref(),
    ) {
        (Some(previous), Some(file_ids)) => preparation
            .staged_mut()
            .rebind_inherited_proof_resolution_source_identities(previous, &publication, file_ids)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to rebind source-identical proof resolution facts: {error}"
                ))
            })?
            .is_some(),
        _ => false,
    };
    if !proof_rebound {
        rematerialize_staged_proof_resolution_projection(
            preparation.staged_mut(),
            &publication,
            cancel_token,
        )?;
    }
    derived_timings.proof_projection_ms = clamp_u128_to_u32(proof_started.elapsed().as_millis());
    let semantic_scope_started = Instant::now();
    let semantic_refresh_scope = incremental_semantic_refresh_scope(
        preparation.staged_mut(),
        root,
        &execution_plan,
        &semantic_plan,
    )?;
    derived_timings.semantic_scope_ms =
        clamp_u128_to_u32(semantic_scope_started.elapsed().as_millis());
    let semantic_projection_started = Instant::now();
    let reused_dense_anchor_projection =
        !stats.graph_projection_changed && !rebuild_complete_dense_anchor_set;
    let semantic_stats = if reused_dense_anchor_projection {
        // Callable, structural, and file fences proved the semantic projection
        // unchanged. The publication stage still rebinds the complete dense
        // anchor manifest to the new core identity; rebuilding selection and
        // graph context for every repository node cannot change any document.
        SemanticProjectionStats::default()
    } else {
        finalize_staged_semantic_docs_for_runtime(
            preparation.staged_mut().store_mut(),
            (!rebuild_complete_dense_anchor_set).then_some(&semantic_refresh_scope),
            (!rebuild_complete_dense_anchor_set).then_some(&semantic_plan.component_reports),
            &source_identity,
            cancel_token,
            runtime,
            SemanticProjectionDocumentSource::SourceFiles {
                max_file_bytes: source_index_policy.byte_cap,
            },
        )?
    };
    derived_timings.semantic_projection_ms =
        clamp_u128_to_u32(semantic_projection_started.elapsed().as_millis());
    ensure_indexing_active(cancel_token)?;
    wall.core_staging_and_mutation = wall
        .core_staging_and_mutation
        .saturating_add(staging_started.elapsed());
    let sealing_started = Instant::now();
    let grounding_started = Instant::now();
    let reused_grounding_snapshots = inherited_grounding_snapshots_ready
        && !stats.graph_projection_changed
        && source_identity_file_ids.is_some();
    let (finalize_stats, detail_snapshot_ms) = if reused_grounding_snapshots {
        preparation
            .staged_mut()
            .store_mut()
            .rebind_grounding_file_snapshots(
                source_identity_file_ids
                    .as_deref()
                    .expect("reused snapshot path requires source identity files"),
            )
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to rebind source-identical grounding snapshots: {error}"
                ))
            })?;
        (
            StagedSnapshotFinalizeStats {
                deferred_indexes_ms: 0,
                summary_snapshot_ms: 0,
            },
            0,
        )
    } else {
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
        (
            finalize_stats,
            clamp_u128_to_u32(detail_started.elapsed().as_millis()),
        )
    };
    derived_timings.grounding_snapshot_ms =
        clamp_u128_to_u32(grounding_started.elapsed().as_millis());
    ensure_indexing_active(cancel_token)?;
    wall.candidate_sealing = sealing_started.elapsed();
    Ok(IncrementalRefreshPreparation::Prepared(Box::new(
        PreparedIncrementalRefresh {
            staged: preparation.release(),
            publication,
            previous_publication,
            stats,
            finalize_stats,
            detail_snapshot_ms,
            semantic_stats,
            reused_dense_anchor_projection,
            source_identity_file_ids,
            retrieval_refresh_receipt,
            semantic_refresh_scope,
            policy_exclusions,
            probe,
            wall,
            derived_timings,
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
    precomputed_probe: Option<IncrementalPlanProbe>,
) -> Result<IndexingRunSummary, ApiError> {
    let core_started = Instant::now();
    let prepared = prepare_incremental_refresh(
        root,
        storage_path,
        events_tx,
        cancel_token,
        runtime,
        source_index_policy,
        precomputed_probe,
    )?;
    let prepared = match prepared {
        IncrementalRefreshPreparation::Unchanged { probe, wall } => {
            return Ok(unchanged_incremental_run_summary(
                probe,
                wall.finish(core_started.elapsed(), None),
            ));
        }
        IncrementalRefreshPreparation::Prepared(prepared) => prepared,
    };
    let PreparedIncrementalRefresh {
        mut staged,
        publication,
        previous_publication,
        stats: index_stats,
        finalize_stats: staged_finalize_stats,
        detail_snapshot_ms,
        semantic_stats: staged_semantic_stats,
        reused_dense_anchor_projection,
        source_identity_file_ids,
        retrieval_refresh_receipt,
        semantic_refresh_scope: llm_refresh_scope,
        policy_exclusions,
        probe,
        mut wall,
        mut derived_timings,
    } = *prepared;
    let staging_started = Instant::now();
    let workspace = match runtime_workspace_manifest(root, storage_path) {
        Ok(workspace) => workspace,
        Err(error) => {
            let _ = staged.discard();
            return Err(ApiError::internal(format!(
                "Failed to reopen project: {error}"
            )));
        }
    };
    let publication_identity_started = Instant::now();
    if let Err(error) = stage_core_publication_identity(
        &mut staged,
        root,
        &workspace,
        &publication,
        &policy_exclusions,
        source_index_policy,
        reused_dense_anchor_projection
            .then_some(previous_publication.as_ref())
            .flatten(),
        source_identity_file_ids.as_deref(),
        cancel_token,
    ) {
        let _ = staged.discard();
        return Err(error);
    }
    derived_timings.publication_identity_ms =
        clamp_u128_to_u32(publication_identity_started.elapsed().as_millis());
    let search_generation_started = Instant::now();
    if !index_stats.graph_projection_changed
        && let Some(previous) = previous_publication.as_ref()
    {
        materialize_equivalent_search_generation(storage_path, previous, &publication)?;
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
    derived_timings.search_generation_ms =
        clamp_u128_to_u32(search_generation_started.elapsed().as_millis());
    if is_indexing_cancelled(cancel_token) {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, &publication);
        return Err(indexing_cancelled_error());
    }
    let prepared_commit =
        PreparedCoreCommit::new(staged, prepared_search_state, storage_path, &publication);
    wall.core_staging_and_mutation = wall
        .core_staging_and_mutation
        .saturating_add(staging_started.elapsed());
    let commit_started = Instant::now();
    let (prepared_search_state, staged_publish_stats, publish_duration) =
        prepared_commit.commit(CoreCommitMode::Incremental, cancel_token)?;
    if let Some(receipt) = retrieval_refresh_receipt {
        codestory_retrieval::install_incremental_retrieval_refresh_receipt(receipt).map_err(
            |error| {
                ApiError::internal(format!(
                    "Failed to retain bounded retrieval refresh evidence: {error}"
                ))
            },
        )?;
    }
    let commit_wall = commit_started.elapsed();
    let mut phase_timings = core_indexing_phase_timings(
        &index_stats,
        staged_finalize_stats,
        detail_snapshot_ms,
        staged_publish_stats,
        publish_duration,
        staged_semantic_stats.semantic_context_index_ms,
    );
    phase_timings.incremental_plan_probe = Some(incremental_plan_probe_timings(&probe));
    phase_timings.incremental_coverage_validation_ms = Some(derived_timings.coverage_validation_ms);
    phase_timings.incremental_proof_projection_ms = Some(derived_timings.proof_projection_ms);
    phase_timings.incremental_semantic_scope_ms = Some(derived_timings.semantic_scope_ms);
    phase_timings.incremental_semantic_projection_ms = Some(derived_timings.semantic_projection_ms);
    phase_timings.incremental_grounding_snapshot_ms = Some(derived_timings.grounding_snapshot_ms);
    phase_timings.incremental_publication_identity_ms =
        Some(derived_timings.publication_identity_ms);
    phase_timings.incremental_search_generation_ms = Some(derived_timings.search_generation_ms);
    phase_timings.incremental_core_wall = Some(wall.finish(
        core_started.elapsed(),
        Some((commit_wall, &staged_publish_stats)),
    ));
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
fn unchanged_incremental_run_summary(
    probe: IncrementalPlanProbe,
    wall: IncrementalCoreWallTimings,
) -> IndexingRunSummary {
    let phase_timings = IndexingPhaseTimings {
        incremental_plan_probe: Some(incremental_plan_probe_timings(&probe)),
        incremental_core_wall: Some(wall),
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
