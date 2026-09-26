use crate::index_commit::{
    CoreCommitMode, PreparedCoreCommit, StagedPreparation, next_index_publication,
    rematerialize_staged_proof_resolution_projection, stage_core_publication_identity,
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
use parking_lot::Mutex;
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

const FAILED_REFRESH_BOUNDARY_LIMIT: usize = 12;
const FAILED_REFRESH_SNAPSHOT_LIMIT: usize = 16;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct FailedRefreshDiagnosticIdentity {
    schema_version: u8,
    activation_operation_id: String,
    activation_attempt: u32,
    activation_revision: u64,
    project_identity: String,
    source_identity: String,
    runtime_configuration_identity: String,
    run_identity: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailedRefreshBoundary {
    FullIndexerReturned,
    FullIndexerError,
    ForwarderJoinBegin,
    ForwarderJoinEnd,
    CoverageBegin,
    CoverageEnd,
    CoverageError,
    ProofBegin,
    ProofEnd,
    ProofError,
    ProofUnfinishedAtCloseout,
    CancellationRequested,
    CancellationObserved,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct FailedRefreshBoundaryRecord {
    identity: FailedRefreshDiagnosticIdentity,
    sequence: u8,
    boundary: FailedRefreshBoundary,
    #[serde(skip_serializing_if = "Option::is_none")]
    cancellation_owner: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_completed_boundary: Option<FailedRefreshBoundary>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct FailedRefreshProofSnapshot {
    identity: FailedRefreshDiagnosticIdentity,
    sequence: u8,
    cache_entries_decoded: u64,
    decoded_cache_bytes: u64,
    source_reauthentication_files: u64,
    source_reauthentication_bytes: u64,
    projection_index_records_prepared: u64,
    calls_resolved: u64,
    dependency_ids_visited: u64,
    correlation_inputs: u64,
    correlation_results: u64,
    facts_sealed: u64,
    facts_persisted: u64,
    proof_store_transaction_started: bool,
    proof_store_transaction_completed: bool,
    cancellation_requested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_observed_cancel_boundary: Option<FailedRefreshBoundary>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct FailedRefreshTerminalRecord {
    identity: FailedRefreshDiagnosticIdentity,
    sequence: u8,
    last_completed_boundary: Option<FailedRefreshBoundary>,
    boundary_records: u8,
    proof_snapshots: u8,
}

#[derive(Debug, Default)]
struct FailedRefreshDiagnosticState {
    identity: Option<FailedRefreshDiagnosticIdentity>,
    sequence: u8,
    boundaries: Vec<FailedRefreshBoundaryRecord>,
    snapshots: Vec<FailedRefreshProofSnapshot>,
    terminal: Option<FailedRefreshTerminalRecord>,
    proof_open: bool,
    cancellation_requested: bool,
    cancellation_observed: bool,
    unavailable: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FailedRefreshDiagnosticSink {
    state: Arc<Mutex<FailedRefreshDiagnosticState>>,
}

impl FailedRefreshDiagnosticSink {
    pub(crate) fn new(identity: FailedRefreshDiagnosticIdentity) -> Self {
        Self {
            state: Arc::new(Mutex::new(FailedRefreshDiagnosticState {
                identity: Some(identity),
                ..Default::default()
            })),
        }
    }

    pub(crate) fn identity(
        activation_operation_id: String,
        activation_attempt: u32,
        activation_revision: u64,
        project_identity: String,
        source_identity: String,
        runtime_configuration_identity: String,
    ) -> FailedRefreshDiagnosticIdentity {
        FailedRefreshDiagnosticIdentity {
            schema_version: 1,
            activation_operation_id,
            activation_attempt,
            activation_revision,
            project_identity,
            source_identity,
            runtime_configuration_identity,
            run_identity: None,
        }
    }

    pub(crate) fn bind_run_identity(&self, run_identity: &str) {
        let Some(mut state) = self.state.try_lock() else {
            return;
        };
        if state.sequence == 0
            && let Some(identity) = state.identity.as_mut()
        {
            identity.run_identity = Some(run_identity.to_owned());
        }
    }

    pub(crate) fn record_boundary(&self, boundary: FailedRefreshBoundary) {
        let Some(mut state) = self.state.try_lock() else {
            return;
        };
        if state.unavailable
            || state.terminal.is_some()
            || state.boundaries.len() >= FAILED_REFRESH_BOUNDARY_LIMIT
        {
            return;
        }
        if state
            .boundaries
            .iter()
            .any(|record| record.boundary == boundary)
        {
            return;
        }
        let last_process_boundary = last_completed_process_boundary(&state);
        let allowed = match boundary {
            FailedRefreshBoundary::FullIndexerReturned
            | FailedRefreshBoundary::FullIndexerError => state.boundaries.is_empty(),
            FailedRefreshBoundary::ForwarderJoinBegin => matches!(
                last_process_boundary,
                Some(
                    FailedRefreshBoundary::FullIndexerReturned
                        | FailedRefreshBoundary::FullIndexerError
                )
            ),
            FailedRefreshBoundary::ForwarderJoinEnd => {
                last_process_boundary == Some(FailedRefreshBoundary::ForwarderJoinBegin)
            }
            FailedRefreshBoundary::CoverageBegin => {
                last_process_boundary == Some(FailedRefreshBoundary::ForwarderJoinEnd)
            }
            FailedRefreshBoundary::CoverageEnd | FailedRefreshBoundary::CoverageError => {
                last_process_boundary == Some(FailedRefreshBoundary::CoverageBegin)
            }
            FailedRefreshBoundary::ProofBegin => {
                last_process_boundary == Some(FailedRefreshBoundary::CoverageEnd)
            }
            FailedRefreshBoundary::ProofEnd
            | FailedRefreshBoundary::ProofError
            | FailedRefreshBoundary::ProofUnfinishedAtCloseout => {
                state.proof_open && last_process_boundary == Some(FailedRefreshBoundary::ProofBegin)
            }
            FailedRefreshBoundary::CancellationRequested => !state.cancellation_requested,
            FailedRefreshBoundary::CancellationObserved => {
                state.cancellation_requested && !state.cancellation_observed
            }
        };
        if !allowed {
            return;
        }
        if boundary == FailedRefreshBoundary::ProofBegin {
            state.proof_open = true;
        } else if matches!(
            boundary,
            FailedRefreshBoundary::ProofEnd
                | FailedRefreshBoundary::ProofError
                | FailedRefreshBoundary::ProofUnfinishedAtCloseout
        ) {
            state.proof_open = false;
        } else if boundary == FailedRefreshBoundary::CancellationRequested {
            state.cancellation_requested = true;
        } else if boundary == FailedRefreshBoundary::CancellationObserved {
            state.cancellation_observed = true;
        }
        let Some(identity) = state.identity.clone() else {
            return;
        };
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        state.boundaries.push(FailedRefreshBoundaryRecord {
            identity,
            sequence,
            boundary,
            cancellation_owner: matches!(
                boundary,
                FailedRefreshBoundary::CancellationRequested
                    | FailedRefreshBoundary::CancellationObserved
            )
            .then_some("activation"),
            last_completed_boundary: last_process_boundary,
        });
    }

    pub(crate) fn observe_cancellation(&self) {
        self.record_boundary(FailedRefreshBoundary::CancellationRequested);
        self.record_boundary(FailedRefreshBoundary::CancellationObserved);
    }

    pub(crate) fn record_proof_progress(
        &self,
        progress: codestory_indexer::ProofResolutionProgress,
        cancellation_requested: bool,
    ) {
        let Some(mut state) = self.state.try_lock() else {
            return;
        };
        if state.unavailable
            || state.terminal.is_some()
            || !state.proof_open
            || state.snapshots.len() >= FAILED_REFRESH_SNAPSHOT_LIMIT
        {
            return;
        }
        let Some(identity) = state.identity.clone() else {
            return;
        };
        let next = FailedRefreshProofSnapshot {
            identity,
            sequence: 0,
            cache_entries_decoded: progress.cache_entries_decoded,
            decoded_cache_bytes: progress.decoded_cache_bytes,
            source_reauthentication_files: progress.source_reauthentication_files,
            source_reauthentication_bytes: progress.source_reauthentication_bytes,
            projection_index_records_prepared: progress.projection_index_records_prepared,
            calls_resolved: progress.calls_resolved,
            dependency_ids_visited: progress.dependency_ids_visited,
            correlation_inputs: progress.correlation_inputs,
            correlation_results: progress.correlation_results,
            facts_sealed: progress.facts_sealed,
            facts_persisted: progress.facts_persisted,
            proof_store_transaction_started: progress.proof_store_transaction_started,
            proof_store_transaction_completed: progress.proof_store_transaction_completed,
            cancellation_requested,
            first_observed_cancel_boundary: cancellation_requested
                .then_some(FailedRefreshBoundary::ProofBegin),
        };
        if let Some(previous) = state.snapshots.last()
            && (!snapshot_is_monotonic(previous, &next) || !snapshot_advances(previous, &next))
        {
            return;
        }
        state.sequence = state.sequence.saturating_add(1);
        let mut next = next;
        next.sequence = state.sequence;
        state.snapshots.push(next);
    }

    pub(crate) fn closeout(&self, cancellation_requested: bool) -> Option<String> {
        if cancellation_requested {
            self.record_boundary(FailedRefreshBoundary::CancellationRequested);
        }
        let proof_open = self.state.try_lock().is_some_and(|state| state.proof_open);
        if proof_open {
            self.record_boundary(FailedRefreshBoundary::ProofUnfinishedAtCloseout);
        }
        let mut state = self.state.try_lock()?;
        if state.unavailable {
            return None;
        }
        if state.terminal.is_none() {
            let identity = state.identity.clone()?;
            state.sequence = state.sequence.saturating_add(1);
            state.terminal = Some(FailedRefreshTerminalRecord {
                identity,
                sequence: state.sequence,
                last_completed_boundary: last_completed_process_boundary(&state),
                boundary_records: state.boundaries.len() as u8,
                proof_snapshots: state.snapshots.len() as u8,
            });
        }
        #[derive(Serialize)]
        struct Report<'a> {
            boundaries: &'a [FailedRefreshBoundaryRecord],
            proof_snapshots: &'a [FailedRefreshProofSnapshot],
            terminal: &'a FailedRefreshTerminalRecord,
        }
        serde_json::to_string(&Report {
            boundaries: &state.boundaries,
            proof_snapshots: &state.snapshots,
            terminal: state.terminal.as_ref().expect("terminal initialized"),
        })
        .ok()
    }

    pub(crate) fn attach_to_error(
        &self,
        mut error: ApiError,
        cancellation_requested: bool,
    ) -> ApiError {
        if error.code == "cancelled" {
            self.observe_cancellation();
        }
        let Some(diagnostic) = self.closeout(cancellation_requested) else {
            return error;
        };
        error.message.push_str("\nfailed_refresh_diagnostic=");
        error.message.push_str(&diagnostic);
        error
    }

    #[cfg(test)]
    fn make_unavailable(&self) {
        if let Some(mut state) = self.state.try_lock() {
            state.unavailable = true;
        }
    }
}

fn last_completed_process_boundary(
    state: &FailedRefreshDiagnosticState,
) -> Option<FailedRefreshBoundary> {
    state
        .boundaries
        .iter()
        .rev()
        .map(|record| record.boundary)
        .find(|boundary| {
            !matches!(
                boundary,
                FailedRefreshBoundary::ProofUnfinishedAtCloseout
                    | FailedRefreshBoundary::CancellationRequested
                    | FailedRefreshBoundary::CancellationObserved
            )
        })
}

fn snapshot_is_monotonic(
    previous: &FailedRefreshProofSnapshot,
    next: &FailedRefreshProofSnapshot,
) -> bool {
    previous.identity == next.identity
        && previous.cache_entries_decoded <= next.cache_entries_decoded
        && previous.decoded_cache_bytes <= next.decoded_cache_bytes
        && previous.source_reauthentication_files <= next.source_reauthentication_files
        && previous.source_reauthentication_bytes <= next.source_reauthentication_bytes
        && previous.projection_index_records_prepared <= next.projection_index_records_prepared
        && previous.calls_resolved <= next.calls_resolved
        && previous.dependency_ids_visited <= next.dependency_ids_visited
        && previous.correlation_inputs <= next.correlation_inputs
        && previous.correlation_results <= next.correlation_results
        && previous.facts_sealed <= next.facts_sealed
        && previous.facts_persisted <= next.facts_persisted
        && (!previous.proof_store_transaction_started || next.proof_store_transaction_started)
        && (!previous.proof_store_transaction_completed || next.proof_store_transaction_completed)
}

fn snapshot_advances(
    previous: &FailedRefreshProofSnapshot,
    next: &FailedRefreshProofSnapshot,
) -> bool {
    previous.cache_entries_decoded < next.cache_entries_decoded
        || previous.decoded_cache_bytes < next.decoded_cache_bytes
        || previous.source_reauthentication_files < next.source_reauthentication_files
        || previous.source_reauthentication_bytes < next.source_reauthentication_bytes
        || previous.projection_index_records_prepared < next.projection_index_records_prepared
        || previous.calls_resolved < next.calls_resolved
        || previous.dependency_ids_visited < next.dependency_ids_visited
        || previous.correlation_inputs < next.correlation_inputs
        || previous.correlation_results < next.correlation_results
        || previous.facts_sealed < next.facts_sealed
        || previous.facts_persisted < next.facts_persisted
        || (!previous.proof_store_transaction_started && next.proof_store_transaction_started)
        || (!previous.proof_store_transaction_completed && next.proof_store_transaction_completed)
        || (!previous.cancellation_requested && next.cancellation_requested)
}

struct FullIndexLiveState {
    previous_publication: Option<IndexPublicationRecord>,
    publication: IndexPublicationRecord,
    dense_anchor_source_identity: String,
    recovering_incomplete_run: bool,
    has_verified_publication: bool,
}

fn incomplete_live_index_requires_recovery(storage_path: &Path) -> Result<bool, ApiError> {
    if !codestory_store::core_database_exists(storage_path).map_err(|error| {
        ApiError::internal(format!("Failed to resolve live core publication: {error}"))
    })? {
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
    let previous_publication =
        if codestory_store::core_database_exists(storage_path).map_err(|error| {
            ApiError::internal(format!("Failed to resolve live core publication: {error}"))
        })? {
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
        .filter(crate::index_coverage::coverage_gap_blocks_publication)
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
    source_index_policy: &SourceIndexPolicy,
) -> Result<PreparedFullRefreshSnapshots, ApiError> {
    let semantic_started = Instant::now();
    let semantic_stats = finalize_staged_semantic_docs_for_runtime(
        staged.store_mut(),
        None,
        None,
        source_identity,
        cancel_token,
        runtime,
        SemanticProjectionDocumentSource::SourceFiles {
            max_file_bytes: source_index_policy.byte_cap,
        },
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

struct FullRefreshIndexerContext<'a> {
    root: &'a Path,
    storage_path: &'a Path,
    events_tx: &'a Sender<AppEventPayload>,
    cancel_token: Option<&'a CancellationToken>,
    source_index_policy: &'a SourceIndexPolicy,
    execution_plan: &'a RefreshExecutionPlan,
    live_state: &'a FullIndexLiveState,
    diagnostics: Option<&'a FailedRefreshDiagnosticSink>,
}

fn run_full_refresh_indexer(
    context: FullRefreshIndexerContext<'_>,
    mut policy_exclusions: Vec<OversizedSourceExclusionCandidate>,
    wall_durations: &mut FullRefreshWallDurations,
) -> Result<FullRefreshIndexingOutput, ApiError> {
    let FullRefreshIndexerContext {
        root,
        storage_path,
        events_tx,
        cancel_token,
        source_index_policy,
        execution_plan,
        live_state,
        diagnostics,
    } = context;
    let stage_started = Instant::now();
    let total_files = execution_plan.files_to_index.len().min(u32::MAX as usize) as u32;
    let _ = events_tx.send(AppEventPayload::IndexingStarted {
        file_count: total_files,
    });
    #[cfg(test)]
    run_source_policy_after_plan_hook();
    // Fresh empty stage (not a live byte-copy). Incremental refresh also
    // escalates here when core CoW cloning is unavailable.
    let staged = SnapshotStore::open_disposable_full_refresh(storage_path)
        .map_err(|error| ApiError::internal(format!("Failed to open staged storage: {error}")))?;
    let mut preparation = StagedPreparation::new(staged);
    #[cfg(test)]
    run_full_refresh_staged_store_hook(preparation.staged_mut().store_mut());
    let staged_proof_fact_count = preparation
        .staged_mut()
        .store_mut()
        .proof_resolution_fact_count()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to inspect the fresh full-refresh proof overlay: {error}"
            ))
        })?;
    let staged_proof_publication = preparation
        .staged_mut()
        .store_mut()
        .get_proof_resolution_publication()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to inspect the fresh full-refresh proof overlay: {error}"
            ))
        })?;
    if staged_proof_fact_count != 0 || staged_proof_publication.is_some() {
        return Err(ApiError::internal(
            "Fresh full-refresh storage contains a proof overlay before graph mutation",
        ));
    }
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
    if let Some(diagnostics) = diagnostics {
        diagnostics.record_boundary(if result.is_ok() {
            FailedRefreshBoundary::FullIndexerReturned
        } else {
            FailedRefreshBoundary::FullIndexerError
        });
    }
    drop(bus);
    if let Some(diagnostics) = diagnostics {
        diagnostics.record_boundary(FailedRefreshBoundary::ForwarderJoinBegin);
    }
    let _ = forwarder.join();
    if let Some(diagnostics) = diagnostics {
        diagnostics.record_boundary(FailedRefreshBoundary::ForwarderJoinEnd);
    }
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
    diagnostics: Option<&FailedRefreshDiagnosticSink>,
) -> Result<PreparedFullRefresh, ApiError> {
    let core_refresh_started = Instant::now();
    let live_started = Instant::now();
    let live_state = inspect_full_index_live_state(root, storage_path, source_index_policy)?;
    if let Some(diagnostics) = diagnostics {
        diagnostics.bind_run_identity(&live_state.publication.run_id);
    }
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
        FullRefreshIndexerContext {
            root,
            storage_path,
            events_tx,
            cancel_token,
            source_index_policy,
            execution_plan: &execution_plan,
            live_state: &live_state,
            diagnostics,
        },
        policy_exclusions,
        &mut wall_durations,
    )?;
    let mut preparation = StagedPreparation::new(output.staged);
    let coverage_started = Instant::now();
    if let Some(diagnostics) = diagnostics {
        diagnostics.record_boundary(FailedRefreshBoundary::CoverageBegin);
    }
    if let Err(error) = validate_full_refresh_coverage(root, preparation.staged_mut(), &live_state)
    {
        if let Some(diagnostics) = diagnostics {
            diagnostics.record_boundary(FailedRefreshBoundary::CoverageError);
        }
        return Err(error);
    }
    if let Some(diagnostics) = diagnostics {
        diagnostics.record_boundary(FailedRefreshBoundary::CoverageEnd);
    }
    wall_durations.coverage_validation = coverage_started.elapsed();
    let copy_started = Instant::now();
    if !live_state.recovering_incomplete_run && live_state.previous_publication.is_some() {
        copy_forward_full_refresh_artifacts(preparation.staged_mut(), storage_path);
    }
    wall_durations.copy_forward = copy_started.elapsed();
    rematerialize_staged_proof_resolution_projection(
        preparation.staged_mut(),
        &live_state.publication,
        cancel_token,
        diagnostics,
    )?;
    let snapshots = prepare_full_refresh_snapshots(
        preparation.staged_mut(),
        &live_state.dense_anchor_source_identity,
        cancel_token,
        runtime,
        source_index_policy,
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

/// Build and publish a from-scratch core database.
///
/// `_annotations_owned` is unused at runtime and load-bearing at compile time:
/// the published database never carried the retained legacy annotation tables,
/// so this function may only be reached from a path that already moved user
/// annotations into the sidecar.
#[allow(clippy::too_many_arguments)]
pub(super) fn index_full_for_runtime(
    root: &Path,
    storage_path: &Path,
    events_tx: &Sender<AppEventPayload>,
    cancel_token: Option<&CancellationToken>,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    source_index_policy: &SourceIndexPolicy,
    _annotations_owned: &crate::controller_bookmarks::AnnotationsOwned,
    diagnostics: Option<&FailedRefreshDiagnosticSink>,
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
        diagnostics,
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
        None,
        None,
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
            discard_unpublished_search_generation(storage_path, publication);
            return Err(error);
        }
    };
    if is_indexing_cancelled(cancel_token) {
        drop(prepared_search_state);
        let _ = staged.discard();
        discard_unpublished_search_generation(storage_path, publication);
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
    crate::activation_retrieval::apply_core_gc_after_publication(
        runtime,
        storage_path,
        cancel_token,
    );
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
        unchanged_publication: false,
        repository_tracking_digest: None,
    })
}

#[cfg(test)]
mod failed_refresh_diagnostic_tests {
    use super::*;

    fn sink() -> FailedRefreshDiagnosticSink {
        FailedRefreshDiagnosticSink::new(FailedRefreshDiagnosticSink::identity(
            "activation-project-1".to_owned(),
            2,
            7,
            "project-opaque".to_owned(),
            "source-opaque".to_owned(),
            "config-opaque".to_owned(),
        ))
    }

    fn through_coverage(sink: &FailedRefreshDiagnosticSink) {
        sink.bind_run_identity("run-opaque");
        sink.record_boundary(FailedRefreshBoundary::FullIndexerReturned);
        sink.record_boundary(FailedRefreshBoundary::ForwarderJoinBegin);
        sink.record_boundary(FailedRefreshBoundary::ForwarderJoinEnd);
        sink.record_boundary(FailedRefreshBoundary::CoverageBegin);
        sink.record_boundary(FailedRefreshBoundary::CoverageEnd);
    }

    #[test]
    fn failed_refresh_diagnostic_orders_success_boundaries_without_changing_wall_timings() {
        let wall = FullRefreshWallDurations {
            live_inspection: Duration::from_millis(1),
            source_discovery: Duration::from_millis(2),
            stage_open: Duration::from_millis(3),
            indexer_execution: Duration::from_millis(4),
            coverage_validation: Duration::from_millis(5),
            copy_forward: Duration::from_millis(6),
            semantic_stage: Duration::from_millis(7),
            snapshot_stage: Duration::from_millis(8),
            publication_prepare: Duration::from_millis(9),
            search_generation: Duration::from_millis(10),
            catalog_publication: Duration::from_millis(11),
        };
        let expected = wall.finish(Duration::from_millis(80));
        let sink = sink();
        through_coverage(&sink);
        sink.record_boundary(FailedRefreshBoundary::ProofBegin);
        sink.record_boundary(FailedRefreshBoundary::ProofBegin);
        sink.record_boundary(FailedRefreshBoundary::ProofEnd);

        let state = sink.state.lock();
        assert_eq!(
            state
                .boundaries
                .iter()
                .map(|record| record.boundary)
                .collect::<Vec<_>>(),
            vec![
                FailedRefreshBoundary::FullIndexerReturned,
                FailedRefreshBoundary::ForwarderJoinBegin,
                FailedRefreshBoundary::ForwarderJoinEnd,
                FailedRefreshBoundary::CoverageBegin,
                FailedRefreshBoundary::CoverageEnd,
                FailedRefreshBoundary::ProofBegin,
                FailedRefreshBoundary::ProofEnd,
            ]
        );
        assert!(state.terminal.is_none());
        assert_eq!(wall.finish(Duration::from_millis(80)), expected);
    }

    #[test]
    fn failed_refresh_diagnostic_coverage_error_stops_before_proof() {
        let sink = sink();
        sink.bind_run_identity("run-opaque");
        sink.record_boundary(FailedRefreshBoundary::FullIndexerReturned);
        sink.record_boundary(FailedRefreshBoundary::ForwarderJoinBegin);
        sink.record_boundary(FailedRefreshBoundary::ForwarderJoinEnd);
        sink.record_boundary(FailedRefreshBoundary::CoverageBegin);
        sink.record_boundary(FailedRefreshBoundary::CoverageError);
        sink.record_boundary(FailedRefreshBoundary::CoverageEnd);
        sink.record_boundary(FailedRefreshBoundary::ProofBegin);
        let _ = sink.closeout(false);

        let state = sink.state.lock();
        assert_eq!(
            state.boundaries.last().map(|record| record.boundary),
            Some(FailedRefreshBoundary::CoverageError)
        );
        assert!(state.snapshots.is_empty());
        assert!(!state.proof_open);
    }

    #[test]
    fn failed_refresh_diagnostic_proof_snapshots_and_cancellation_are_monotonic() {
        let sink = sink();
        through_coverage(&sink);
        sink.record_boundary(FailedRefreshBoundary::ProofBegin);
        for calls_resolved in [1, 3, 2, 8] {
            sink.record_proof_progress(
                codestory_indexer::ProofResolutionProgress {
                    calls_resolved,
                    ..Default::default()
                },
                calls_resolved == 8,
            );
        }
        let _ = sink.attach_to_error(ApiError::new("cancelled", "indexing cancelled"), true);

        let state = sink.state.lock();
        assert_eq!(
            state
                .snapshots
                .iter()
                .map(|snapshot| snapshot.calls_resolved)
                .collect::<Vec<_>>(),
            vec![1, 3, 8]
        );
        let boundaries = state
            .boundaries
            .iter()
            .map(|record| record.boundary)
            .collect::<Vec<_>>();
        let requested = boundaries
            .iter()
            .position(|boundary| *boundary == FailedRefreshBoundary::CancellationRequested)
            .expect("cancellation requested");
        let observed = boundaries
            .iter()
            .position(|boundary| *boundary == FailedRefreshBoundary::CancellationObserved)
            .expect("cancellation observed");
        let unfinished = boundaries
            .iter()
            .position(|boundary| *boundary == FailedRefreshBoundary::ProofUnfinishedAtCloseout)
            .expect("proof unfinished");
        assert!(requested < observed && observed < unfinished);
        assert!(!boundaries.contains(&FailedRefreshBoundary::ProofEnd));
        assert_eq!(
            state
                .terminal
                .as_ref()
                .expect("terminal")
                .last_completed_boundary,
            Some(FailedRefreshBoundary::ProofBegin)
        );
    }

    #[test]
    fn failed_refresh_diagnostic_sink_failure_preserves_original_error() {
        let sink = sink();
        sink.make_unavailable();
        let original = ApiError::new("source_coverage_incomplete", "coverage failed");
        let observed = sink.attach_to_error(original.clone(), false);

        assert_eq!(observed, original);
    }

    #[test]
    fn failed_refresh_diagnostic_is_bounded_private_and_terminally_immutable() {
        let sink = sink();
        through_coverage(&sink);
        sink.record_boundary(FailedRefreshBoundary::ProofBegin);
        for calls_resolved in 0..32 {
            sink.record_proof_progress(
                codestory_indexer::ProofResolutionProgress {
                    calls_resolved,
                    ..Default::default()
                },
                false,
            );
        }
        let report = sink.closeout(false).expect("diagnostic report");
        let terminal_sequence = sink
            .state
            .lock()
            .terminal
            .as_ref()
            .expect("terminal")
            .sequence;
        for _ in 0..8 {
            sink.record_boundary(FailedRefreshBoundary::ProofError);
            sink.record_proof_progress(
                codestory_indexer::ProofResolutionProgress {
                    calls_resolved: u64::MAX,
                    ..Default::default()
                },
                true,
            );
        }

        let state = sink.state.lock();
        assert!(state.boundaries.len() <= FAILED_REFRESH_BOUNDARY_LIMIT);
        assert_eq!(state.snapshots.len(), FAILED_REFRESH_SNAPSHOT_LIMIT);
        assert_eq!(
            state.terminal.as_ref().expect("terminal").sequence,
            terminal_sequence
        );
        assert!(!report.contains("/Users/"));
        assert!(!report.contains("query"));
        assert!(!report.contains("callsite"));
        assert!(!report.contains("file_path"));
        assert!(!report.contains("source_text"));
        assert!(serde_json::from_str::<serde_json::Value>(&report).is_ok());
    }

    #[test]
    fn failed_refresh_diagnostic_rejects_concurrent_duplicates_and_late_records() {
        let sink = sink();
        through_coverage(&sink);
        sink.record_boundary(FailedRefreshBoundary::ProofBegin);
        let handles = (0..8)
            .map(|_| {
                let sink = sink.clone();
                std::thread::spawn(move || {
                    sink.record_proof_progress(
                        codestory_indexer::ProofResolutionProgress {
                            calls_resolved: 1,
                            ..Default::default()
                        },
                        false,
                    );
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().expect("diagnostic writer");
        }
        let _ = sink.closeout(false);
        let (boundary_count, snapshot_count, terminal_sequence) = {
            let state = sink.state.lock();
            (
                state.boundaries.len(),
                state.snapshots.len(),
                state.terminal.as_ref().expect("terminal").sequence,
            )
        };
        sink.record_boundary(FailedRefreshBoundary::ProofError);
        sink.record_proof_progress(
            codestory_indexer::ProofResolutionProgress {
                calls_resolved: 2,
                ..Default::default()
            },
            false,
        );

        let state = sink.state.lock();
        assert_eq!(snapshot_count, 1);
        assert_eq!(state.boundaries.len(), boundary_count);
        assert_eq!(state.snapshots.len(), snapshot_count);
        assert_eq!(
            state.terminal.as_ref().expect("terminal").sequence,
            terminal_sequence
        );
    }
}
