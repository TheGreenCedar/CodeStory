use crate::index_commit::{
    CoreCommitMode, PreparedCoreCommit, StagedPreparation, next_index_publication,
    stage_core_publication_identity,
};
use crate::index_incremental::spawn_progress_forwarder;
use crate::index_timings::{
    FullRefreshWallDurations, IndexingRunSummary, apply_full_refresh_pipeline_timings,
    core_indexing_phase_timings,
};
#[cfg(test)]
use crate::publication::{run_full_refresh_staged_store_hook, run_source_policy_after_plan_hook};
use crate::search_publication::discard_unpublished_search_generation;
use crate::search_state_cache::{
    ensure_indexing_active, indexing_cancelled_error, is_indexing_cancelled,
    rebuild_search_state_from_storage_for_runtime,
};
use crate::semantic_projection::{
    SemanticProjectionDocumentSource, SemanticProjectionStats,
    finalize_staged_semantic_docs_for_runtime,
};
use crate::workspace_state::runtime_workspace_manifest;
use crate::{
    clamp_u128_to_u32, full_refresh_execution_plan_with_coverage, source_coverage_failure_code,
    stored_file_coverage_diagnostics, validate_source_policy_exclusions,
    validate_structural_text_units,
};
use codestory_contracts::api::{ApiError, AppEventPayload};
use codestory_contracts::events::EventBus;
use codestory_contracts::graph::FileCoverageReason;
use codestory_indexer::{
    ArtifactCachePolicies, ArtifactCachePolicy, CancellationToken, IncrementalIndexingStats,
    WorkspaceIndexer as V2WorkspaceIndexer,
};
use codestory_store::{
    IndexPublicationMode, IndexPublicationRecord, SnapshotStore, StagedSnapshot,
    StagedSnapshotFinalizeStats, Store,
};
use codestory_workspace::{
    OversizedSourceExclusionCandidate, RefreshExecutionPlan, SourceIndexPolicy, WorkspaceManifest,
};
use crossbeam_channel::Sender;
use std::path::Path;
use std::time::{Duration, Instant};
use uuid::Uuid;

struct FullIndexLiveState {
    previous_publication: Option<IndexPublicationRecord>,
    publication: IndexPublicationRecord,
    dense_anchor_source_identity: String,
    recovering_incomplete_run: bool,
    has_verified_publication: bool,
}

fn incomplete_live_index_requires_recovery(storage_path: &Path) -> Result<bool, ApiError> {
    if !storage_path.exists() {
        return Ok(false);
    }
    match Store::database_schema_version(storage_path) {
        Ok(version) if version > codestory_store::CURRENT_SCHEMA_VERSION => {
            Store::database_has_incomplete_incremental_run(storage_path).map_err(|error| {
                ApiError::internal(format!("Failed to inspect live storage: {error}"))
            })
        }
        Ok(_) => match Store::database_has_incomplete_incremental_run(storage_path) {
            Ok(marked) => Ok(marked),
            Err(error) => {
                tracing::warn!(
                    path = %storage_path.display(),
                    "Live storage could not be inspected; rebuilding without copying derived state: {error}"
                );
                Ok(true)
            }
        },
        Err(error) => {
            tracing::warn!(
                path = %storage_path.display(),
                "Live storage schema could not be read; rebuilding without copying derived state: {error}"
            );
            Ok(true)
        }
    }
}

fn live_publication_is_verified(
    root: &Path,
    storage_path: &Path,
    expected: Option<&IndexPublicationRecord>,
    recovering_incomplete_run: bool,
    source_index_policy: &SourceIndexPolicy,
) -> bool {
    if recovering_incomplete_run {
        return false;
    }
    let Some(expected) = expected else {
        return false;
    };
    let live = match Store::open_read_only(storage_path) {
        Ok(storage) => storage,
        Err(error) => {
            tracing::debug!(
                path = %storage_path.display(),
                "Live publication could not be opened for verification: {error}"
            );
            return false;
        }
    };
    let publication = match live.get_complete_index_publication() {
        Ok(Some(publication)) if publication == *expected => publication,
        Ok(_) => return false,
        Err(error) => {
            tracing::debug!(
                path = %storage_path.display(),
                "Live core publication could not be verified: {error}"
            );
            return false;
        }
    };
    if let Err(error) = live.validate_dense_anchor_publication(&publication) {
        tracing::debug!(
            path = %storage_path.display(),
            "Live dense anchor publication could not be verified: {error}"
        );
        return false;
    }
    validate_structural_text_units(&live, &publication).is_ok()
        && validate_source_policy_exclusions(&live, root, &publication, source_index_policy).is_ok()
}

fn inspect_full_index_live_state(
    root: &Path,
    storage_path: &Path,
    source_index_policy: &SourceIndexPolicy,
) -> Result<FullIndexLiveState, ApiError> {
    let previous_publication = if storage_path.exists() {
        Store::database_index_publication(storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to inspect live publication identity: {error}"
            ))
        })?
    } else {
        None
    };
    let publication = next_index_publication(
        previous_publication.as_ref(),
        IndexPublicationMode::Full,
        &Uuid::new_v4().to_string(),
    )?;
    let dense_anchor_source_identity =
        format!("core:{}:{}", publication.generation_id, publication.run_id);
    let recovering_incomplete_run = incomplete_live_index_requires_recovery(storage_path)?;
    let has_verified_publication = live_publication_is_verified(
        root,
        storage_path,
        previous_publication.as_ref(),
        recovering_incomplete_run,
        source_index_policy,
    );
    Ok(FullIndexLiveState {
        previous_publication,
        publication,
        dense_anchor_source_identity,
        recovering_incomplete_run,
        has_verified_publication,
    })
}

fn validate_full_refresh_coverage(
    root: &Path,
    staged: &mut StagedSnapshot,
    live_state: &FullIndexLiveState,
) -> Result<(), ApiError> {
    let blocking_gaps = stored_file_coverage_diagnostics(root, staged.store_mut())?
        .into_iter()
        .filter(|entry| entry.reason != FileCoverageReason::ParserPartial)
        .collect::<Vec<_>>();
    if blocking_gaps.is_empty() {
        return Ok(());
    }
    let sample = blocking_gaps
        .iter()
        .take(3)
        .map(|entry| format!("{} ({})", entry.path, entry.reason.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    let remainder = blocking_gaps.len().saturating_sub(3);
    let sample = if remainder > 0 {
        format!("{sample}, and {remainder} more")
    } else {
        sample
    };
    let preserved_state = if live_state.has_verified_publication {
        "The previous complete publication was preserved"
    } else if live_state.recovering_incomplete_run {
        "The existing live index and its incomplete-run recovery fence were preserved"
    } else if live_state.previous_publication.is_some() {
        "The existing live index was preserved and no replacement publication was created"
    } else {
        "No core publication was created"
    };
    let count = blocking_gaps.len();
    Err(ApiError::source_coverage_failure(
        source_coverage_failure_code(&blocking_gaps),
        format!(
            "Effective refresh mode `full` could not verify {count} scheduled file(s): {sample}. {preserved_state}."
        ),
        blocking_gaps,
    ))
}

fn copy_forward_full_refresh_artifacts(staged: &mut StagedSnapshot, storage_path: &Path) {
    match staged
        .store_mut()
        .copy_retrieval_artifact_nodes_from(storage_path)
    {
        Ok(copied) => tracing::debug!(
            copied,
            "Copied retrieval artifact nodes into staged storage"
        ),
        Err(error) => {
            tracing::warn!("Failed to copy retrieval artifact nodes into staged storage: {error}")
        }
    }
    match staged
        .store_mut()
        .copy_symbol_search_docs_from(storage_path)
    {
        Ok(copied) => tracing::debug!(copied, "Copied symbol docs into staged storage"),
        Err(error) => tracing::warn!("Failed to copy symbol docs into staged storage: {error}"),
    }
    match staged
        .store_mut()
        .copy_dense_anchor_inputs_from(storage_path)
    {
        Ok(copied) => tracing::debug!(copied, "Copied dense anchor inputs into staged storage"),
        Err(error) => {
            tracing::warn!("Failed to copy dense anchor inputs into staged storage: {error}")
        }
    }
}

struct PreparedFullRefreshSnapshots {
    semantic_stats: SemanticProjectionStats,
    finalize_stats: StagedSnapshotFinalizeStats,
    detail_snapshot_ms: u32,
    semantic_duration: Duration,
    snapshot_duration: Duration,
}

fn prepare_full_refresh_snapshots(
    staged: &mut StagedSnapshot,
    source_identity: &str,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<PreparedFullRefreshSnapshots, ApiError> {
    let semantic_started = Instant::now();
    let semantic_stats = finalize_staged_semantic_docs_for_runtime(
        staged.store_mut(),
        None,
        None,
        source_identity,
        cancel_token,
        runtime,
        SemanticProjectionDocumentSource::SourceFiles,
    )?;
    ensure_indexing_active(cancel_token)?;
    let semantic_duration = semantic_started.elapsed();
    let snapshot_started = Instant::now();
    let finalize_stats = staged.snapshots().finalize_staged().map_err(|error| {
        ApiError::internal(format!(
            "Failed to finalize staged snapshot lifecycle: {error}"
        ))
    })?;
    let detail_started = Instant::now();
    staged.snapshots().refresh_detail().map_err(|error| {
        ApiError::internal(format!(
            "Failed to finalize staged detail snapshots: {error}"
        ))
    })?;
    ensure_indexing_active(cancel_token)?;
    Ok(PreparedFullRefreshSnapshots {
        semantic_stats,
        finalize_stats,
        detail_snapshot_ms: clamp_u128_to_u32(detail_started.elapsed().as_millis()),
        semantic_duration,
        snapshot_duration: snapshot_started.elapsed(),
    })
}

struct FullRefreshIndexingOutput {
    staged: StagedSnapshot,
    stats: IncrementalIndexingStats,
    policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
}

fn run_full_refresh_indexer(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    source_index_policy: &SourceIndexPolicy,
    execution_plan: &RefreshExecutionPlan,
    mut policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
    live_state: &FullIndexLiveState,
    wall_durations: &mut FullRefreshWallDurations,
) -> Result<FullRefreshIndexingOutput, ApiError> {
    let stage_started = Instant::now();
    let total_files = execution_plan.files_to_index.len().min(u32::MAX as usize) as u32;
    let _ = events_tx.send(AppEventPayload::IndexingStarted {
        file_count: total_files,
    });
    #[cfg(test)]
    run_source_policy_after_plan_hook();
    let staged = SnapshotStore::open_disposable_full_refresh(storage_path)
        .map_err(|error| ApiError::internal(format!("Failed to open staged storage: {error}")))?;
    let mut preparation = StagedPreparation::new(staged);
    #[cfg(test)]
    run_full_refresh_staged_store_hook(preparation.staged_mut().store_mut());
    let copied_structural_artifacts = if live_state.has_verified_publication {
        match preparation
            .staged_mut()
            .store_mut()
            .copy_structural_text_artifact_cache_from(storage_path)
        {
            Ok(copied) => {
                tracing::debug!(
                    copied,
                    "Copied verified structural artifacts into staged storage"
                );
                copied
            }
            Err(error) => {
                tracing::warn!(
                    "Failed to copy verified structural artifacts into staged storage; recollecting: {error}"
                );
                0
            }
        }
    } else {
        0
    };
    let bus = EventBus::new();
    let forwarder = spawn_progress_forwarder(bus.receiver(), events_tx.clone());
    let indexer = V2WorkspaceIndexer::new(root.to_path_buf())
        .with_source_index_policy(source_index_policy.clone())
        .with_artifact_cache_policies(ArtifactCachePolicies {
            parser: ArtifactCachePolicy::KnownEmpty,
            structural: if copied_structural_artifacts > 0 {
                ArtifactCachePolicy::ReadThrough
            } else {
                ArtifactCachePolicy::KnownEmpty
            },
        });
    wall_durations.stage_open = stage_started.elapsed();
    let execution_started = Instant::now();
    let result = indexer.run_with_policy_exclusions(
        preparation.staged_mut().store_mut(),
        execution_plan,
        &bus,
        cancel_token,
    );
    drop(bus);
    let _ = forwarder.join();
    let outcome = match result {
        Ok(_) if is_indexing_cancelled(cancel_token) => {
            return Err(indexing_cancelled_error());
        }
        Ok(outcome) => outcome,
        Err(_) if is_indexing_cancelled(cancel_token) => {
            return Err(indexing_cancelled_error());
        }
        Err(error) => return Err(ApiError::internal(format!("Indexing failed: {error}"))),
    };
    wall_durations.indexer_execution = execution_started.elapsed();
    policy_exclusions.extend(outcome.policy_exclusions);
    Ok(FullRefreshIndexingOutput {
        staged: preparation.release(),
        stats: outcome.stats,
        policy_exclusions,
    })
}

struct PreparedFullRefresh {
    staged: StagedSnapshot,
    live_state: FullIndexLiveState,
    workspace: WorkspaceManifest,
    stats: IncrementalIndexingStats,
    policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
    snapshots: PreparedFullRefreshSnapshots,
    wall_durations: FullRefreshWallDurations,
    core_refresh_started: Instant,
}

fn prepare_full_refresh(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
) -> Result<PreparedFullRefresh, ApiError> {
    let core_refresh_started = Instant::now();
    let live_started = Instant::now();
    let live_state = inspect_full_index_live_state(root, storage_path, source_index_policy)?;
    let mut wall_durations = FullRefreshWallDurations {
        live_inspection: live_started.elapsed(),
        ..Default::default()
    };
    let discovery_started = Instant::now();
    let workspace = runtime_workspace_manifest(root, storage_path)
        .map_err(|error| ApiError::internal(format!("Failed to open project: {error}")))?;
    let (execution_plan, policy_exclusions) =
        full_refresh_execution_plan_with_coverage(root, &workspace, source_index_policy)?;
    wall_durations.source_discovery = discovery_started.elapsed();
    let output = run_full_refresh_indexer(
        root,
        storage_path,
        events_tx,
        cancel_token,
        source_index_policy,
        &execution_plan,
        policy_exclusions,
        &live_state,
        &mut wall_durations,
    )?;
    let mut preparation = StagedPreparation::new(output.staged);
    let coverage_started = Instant::now();
    validate_full_refresh_coverage(root, preparation.staged_mut(), &live_state)?;
    wall_durations.coverage_validation = coverage_started.elapsed();
    let copy_started = Instant::now();
    if !live_state.recovering_incomplete_run && storage_path.exists() {
        copy_forward_full_refresh_artifacts(preparation.staged_mut(), storage_path);
    }
    wall_durations.copy_forward = copy_started.elapsed();
    let snapshots = prepare_full_refresh_snapshots(
        preparation.staged_mut(),
        &live_state.dense_anchor_source_identity,
        cancel_token,
        runtime,
    )?;
    wall_durations.semantic_stage = snapshots.semantic_duration;
    wall_durations.snapshot_stage = snapshots.snapshot_duration;
    Ok(PreparedFullRefresh {
        staged: preparation.release(),
        live_state,
        workspace,
        stats: output.stats,
        policy_exclusions: output.policy_exclusions,
        snapshots,
        wall_durations,
        core_refresh_started,
    })
}

pub(super) fn index_full_for_runtime(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
) -> Result<IndexingRunSummary, ApiError> {
    let PreparedFullRefresh {
        mut staged,
        live_state,
        workspace,
        stats: index_stats,
        policy_exclusions,
        snapshots: prepared_snapshots,
        mut wall_durations,
        core_refresh_started,
    } = prepare_full_refresh(
        root,
        storage_path,
        events_tx,
        cancel_token,
        runtime,
        source_index_policy,
    )?;
    let publication = &live_state.publication;
    let recovering_incomplete_run = live_state.recovering_incomplete_run;
    let mut wall_stage_started = Instant::now();
    if recovering_incomplete_run && let Err(err) = staged.store_mut().begin_incremental_run() {
        let _ = staged.discard();
        return Err(ApiError::internal(format!(
            "Failed to preserve incomplete marker through staged recovery: {err}"
        )));
    }
    if let Err(error) = stage_core_publication_identity(
        &mut staged,
        root,
        &workspace,
        publication,
        &policy_exclusions,
        source_index_policy,
        cancel_token,
    ) {
        let _ = staged.discard();
        return Err(error);
    }
    wall_durations.publication_prepare = wall_stage_started.elapsed();
    wall_stage_started = Instant::now();
    let prepared_search_state = match rebuild_search_state_from_storage_for_runtime(
        staged.store_mut(),
        storage_path,
        None,
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
    wall_durations.search_generation = wall_stage_started.elapsed();
    wall_stage_started = Instant::now();
    let prepared_commit =
        PreparedCoreCommit::new(staged, prepared_search_state, storage_path, publication);
    let (prepared_search_state, staged_publish_stats, publish_duration) = prepared_commit.commit(
        CoreCommitMode::Full {
            finish_recovery_marker: recovering_incomplete_run,
        },
        cancel_token,
    )?;
    wall_durations.catalog_publication = wall_stage_started.elapsed();
    let full_refresh_wall = wall_durations.finish(core_refresh_started.elapsed());
    let mut phase_timings = core_indexing_phase_timings(
        &index_stats,
        prepared_snapshots.finalize_stats,
        prepared_snapshots.detail_snapshot_ms,
        staged_publish_stats,
        publish_duration,
        prepared_snapshots.semantic_stats.semantic_context_index_ms,
    );
    apply_full_refresh_pipeline_timings(&mut phase_timings, &index_stats, full_refresh_wall);
    Ok(IndexingRunSummary {
        phase_timings,
        staged_semantic_stats: prepared_snapshots.semantic_stats,
        llm_refresh_scope: None,
        #[cfg(test)]
        publication: publication.clone(),
        prepared_search_state: Some(prepared_search_state),
    })
}
