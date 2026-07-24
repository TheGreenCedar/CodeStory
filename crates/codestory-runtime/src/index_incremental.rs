use crate::index_commit::{
    CoreCommitMode, PreparedCoreCommit, StagedPreparation, next_index_publication,
    stage_core_publication_identity,
};
use crate::index_timings::{IndexingRunSummary, core_indexing_phase_timings};
use crate::search_publication::discard_unpublished_search_generation;
use crate::search_state_cache::{
    ensure_indexing_active, indexing_cancelled_error, is_indexing_cancelled,
    rebuild_search_state_from_storage_for_runtime, workspace_refresh_inputs,
};
use crate::semantic_projection::{
    ComponentReportRefreshScope, SemanticProjectionDocumentSource, SemanticProjectionStats,
    finalize_staged_semantic_docs_for_runtime, semantic_component_key_for_path,
    semantic_file_table_path_map, semantic_graph_dependent_file_ids_by_seed,
};
use crate::workspace_state::runtime_workspace_manifest;
use crate::{
    clamp_u128_to_u32, file_coverage_retryable, runtime_relative_path,
    source_coverage_failure_code, stored_file_coverage_diagnostics,
};
#[cfg(test)]
use crate::{publication::run_incremental_staged_store_hook, test_sidecar_runtime_from_env};
use codestory_contracts::api::{
    ApiError, ApiErrorDetails, AppEventPayload, FileCoverageDiagnosticDto,
};
use codestory_contracts::events::{Event, EventBus};
use codestory_contracts::graph::FileCoverageReason;
use codestory_indexer::{
    CancellationToken, IncrementalIndexingStats, WorkspaceIndexer as V2WorkspaceIndexer,
};
use codestory_store::{
    CURRENT_SCHEMA_VERSION, IndexPublicationMode, IndexPublicationRecord, SnapshotStore,
    StagedSnapshot, StagedSnapshotFinalizeStats, Store,
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
    )
}

pub(super) fn index_incremental_for_runtime(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
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
}

fn prepare_incremental_refresh(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
) -> Result<PreparedIncrementalRefresh, ApiError> {
    ensure_incremental_refresh_compatible(root, storage_path)?;
    ensure_indexing_active(cancel_token)?;
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
        SemanticProjectionDocumentSource::SourceFiles,
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
    Ok(PreparedIncrementalRefresh {
        staged: preparation.release(),
        publication,
        stats,
        finalize_stats,
        detail_snapshot_ms: clamp_u128_to_u32(detail_started.elapsed().as_millis()),
        semantic_stats,
        semantic_refresh_scope,
        policy_exclusions,
    })
}

fn run_incremental_indexing_common(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
) -> Result<IndexingRunSummary, ApiError> {
    let PreparedIncrementalRefresh {
        mut staged,
        publication,
        stats: index_stats,
        finalize_stats: staged_finalize_stats,
        detail_snapshot_ms,
        semantic_stats: staged_semantic_stats,
        semantic_refresh_scope: llm_refresh_scope,
        policy_exclusions,
    } = prepare_incremental_refresh(
        root,
        storage_path,
        events_tx,
        cancel_token,
        runtime,
        source_index_policy,
    )?;
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
    let phase_timings = core_indexing_phase_timings(
        &index_stats,
        staged_finalize_stats,
        detail_snapshot_ms,
        staged_publish_stats,
        publish_duration,
        staged_semantic_stats.semantic_context_index_ms,
    );
    Ok(IndexingRunSummary {
        phase_timings,
        staged_semantic_stats,
        llm_refresh_scope: Some(llm_refresh_scope),
        #[cfg(test)]
        publication,
        prepared_search_state: Some(prepared_search_state),
    })
}
