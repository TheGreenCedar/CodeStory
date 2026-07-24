use crate::semantic_projection::{SearchStateBuildResult, SemanticProjectionStats};
use crate::{clamp_u64_to_u32, clamp_u128_to_u32, clamp_usize_to_u32};
use codestory_contracts::api::{
    ArtifactCacheAccessTimings, ArtifactCachePolicyDto, CorePromotionTimings,
    DatabaseSnapshotCopyTimings, FullRefreshWallTimings, IndexingPhaseTimings,
    ProjectionPersistenceFamilyTimings, ProjectionPersistenceTimings,
};
use codestory_indexer::{ArtifactCacheFamilyStats, ArtifactCachePolicy, IncrementalIndexingStats};
#[cfg(test)]
use codestory_store::IndexPublicationRecord;
use codestory_store::{StagedSnapshotFinalizeStats, StagedSnapshotPublishStats};
use std::collections::HashSet;
use std::time::Duration;

pub(super) struct IndexingRunSummary {
    pub(super) phase_timings: IndexingPhaseTimings,
    pub(super) staged_semantic_stats: SemanticProjectionStats,
    pub(super) llm_refresh_scope: Option<HashSet<codestory_contracts::graph::NodeId>>,
    #[cfg(test)]
    pub(super) publication: IndexPublicationRecord,
    pub(super) prepared_search_state: Option<SearchStateBuildResult>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct FullRefreshWallDurations {
    pub(super) live_inspection: Duration,
    pub(super) source_discovery: Duration,
    pub(super) stage_open: Duration,
    pub(super) indexer_execution: Duration,
    pub(super) coverage_validation: Duration,
    pub(super) copy_forward: Duration,
    pub(super) semantic_stage: Duration,
    pub(super) snapshot_stage: Duration,
    pub(super) publication_prepare: Duration,
    pub(super) search_generation: Duration,
    pub(super) catalog_publication: Duration,
}

impl FullRefreshWallDurations {
    pub(super) fn finish(self, core_refresh: Duration) -> FullRefreshWallTimings {
        let accounted = [
            self.live_inspection,
            self.source_discovery,
            self.stage_open,
            self.indexer_execution,
            self.coverage_validation,
            self.copy_forward,
            self.semantic_stage,
            self.snapshot_stage,
            self.publication_prepare,
            self.search_generation,
            self.catalog_publication,
        ]
        .into_iter()
        .fold(Duration::ZERO, Duration::saturating_add);
        let unattributed = core_refresh.saturating_sub(accounted);

        FullRefreshWallTimings {
            core_refresh_ms: clamp_u128_to_u32(core_refresh.as_millis()),
            live_inspection_ms: clamp_u128_to_u32(self.live_inspection.as_millis()),
            source_discovery_ms: clamp_u128_to_u32(self.source_discovery.as_millis()),
            stage_open_ms: clamp_u128_to_u32(self.stage_open.as_millis()),
            indexer_execution_ms: clamp_u128_to_u32(self.indexer_execution.as_millis()),
            coverage_validation_ms: clamp_u128_to_u32(self.coverage_validation.as_millis()),
            copy_forward_ms: clamp_u128_to_u32(self.copy_forward.as_millis()),
            semantic_stage_ms: clamp_u128_to_u32(self.semantic_stage.as_millis()),
            snapshot_stage_ms: clamp_u128_to_u32(self.snapshot_stage.as_millis()),
            publication_prepare_ms: clamp_u128_to_u32(self.publication_prepare.as_millis()),
            search_generation_ms: clamp_u128_to_u32(self.search_generation.as_millis()),
            catalog_publication_ms: clamp_u128_to_u32(self.catalog_publication.as_millis()),
            unattributed_ms: clamp_u128_to_u32(unattributed.as_millis()),
        }
    }
}

fn apply_core_indexing_stats(timings: &mut IndexingPhaseTimings, stats: &IncrementalIndexingStats) {
    timings.parse_index_ms = clamp_u64_to_u32(stats.parse_index_ms);
    timings.projection_flush_ms = clamp_u64_to_u32(stats.projection_flush_ms);
    timings.edge_resolution_ms = clamp_u64_to_u32(stats.edge_resolution_ms);
    timings.error_flush_ms = clamp_u64_to_u32(stats.error_flush_ms);
    timings.cleanup_ms = clamp_u64_to_u32(stats.cleanup_ms);
    timings.artifact_cache_write_ms = Some(clamp_u64_to_u32(stats.artifact_cache_write_ms));
    timings.artifact_cache_writes = Some(clamp_usize_to_u32(stats.artifact_cache_writes));
    timings.artifact_cache_write_transactions =
        Some(clamp_usize_to_u32(stats.artifact_cache_write_transactions));
    timings.parser_artifact_cache =
        Some(artifact_cache_access_timings(&stats.parser_artifact_cache));
    timings.structural_artifact_cache = Some(artifact_cache_access_timings(
        &stats.structural_artifact_cache,
    ));
    timings.source_prepare_ms = Some(clamp_u64_to_u32(stats.source_prepare_ms));
    timings.projection_batch_wall_ms = Some(clamp_u64_to_u32(stats.projection_batch_wall_ms));
    timings.projection_batch_transactions =
        Some(clamp_usize_to_u32(stats.projection_batch_transactions));
    timings.projection_persistence = Some(projection_persistence_timings(
        &stats.projection_persistence,
    ));
    timings.unresolved_calls_start = clamp_usize_to_u32(stats.unresolved_calls_start);
    timings.unresolved_imports_start = clamp_usize_to_u32(stats.unresolved_imports_start);
    timings.resolved_calls = clamp_usize_to_u32(stats.resolved_calls);
    timings.resolved_imports = clamp_usize_to_u32(stats.resolved_imports);
    timings.unresolved_calls_end = clamp_usize_to_u32(stats.unresolved_calls_end);
    timings.unresolved_imports_end = clamp_usize_to_u32(stats.unresolved_imports_end);
}

fn apply_resolution_telemetry(
    timings: &mut IndexingPhaseTimings,
    telemetry: &OptionalResolutionTelemetry,
) {
    timings.setup_existing_projection_ids_ms = telemetry.setup_existing_projection_ids_ms;
    timings.setup_seed_symbol_table_ms = telemetry.setup_seed_symbol_table_ms;
    timings.flush_files_ms = telemetry.flush_files_ms;
    timings.flush_nodes_ms = telemetry.flush_nodes_ms;
    timings.flush_edges_ms = telemetry.flush_edges_ms;
    timings.flush_occurrences_ms = telemetry.flush_occurrences_ms;
    timings.flush_component_access_ms = telemetry.flush_component_access_ms;
    timings.flush_callable_projection_ms = telemetry.flush_callable_projection_ms;
    timings.resolution_override_count_ms = telemetry.resolution_override_count_ms;
    timings.resolution_unresolved_counts_ms = telemetry.resolution_unresolved_counts_ms;
    timings.resolution_calls_ms = telemetry.resolution_calls_ms;
    timings.resolution_imports_ms = telemetry.resolution_imports_ms;
    timings.resolution_cleanup_ms = telemetry.resolution_cleanup_ms;
    timings.resolution_call_candidate_index_ms = telemetry.resolution_call_candidate_index_ms;
    timings.resolution_import_candidate_index_ms = telemetry.resolution_import_candidate_index_ms;
    timings.resolution_call_semantic_index_ms = telemetry.resolution_call_semantic_index_ms;
    timings.resolution_import_semantic_index_ms = telemetry.resolution_import_semantic_index_ms;
    timings.resolution_support_snapshot_limit_bytes =
        telemetry.resolution_support_snapshot_limit_bytes;
    timings.resolution_support_snapshot_stored = telemetry.resolution_support_snapshot_stored;
    timings.resolution_support_snapshot_skipped_oversize =
        telemetry.resolution_support_snapshot_skipped_oversize;
    timings.resolution_call_semantic_candidates_ms =
        telemetry.resolution_call_semantic_candidates_ms;
    timings.resolution_import_semantic_candidates_ms =
        telemetry.resolution_import_semantic_candidates_ms;
    timings.resolution_call_semantic_requests = telemetry.resolution_call_semantic_requests;
    timings.resolution_call_semantic_unique_requests =
        telemetry.resolution_call_semantic_unique_requests;
    timings.resolution_call_semantic_skipped_requests =
        telemetry.resolution_call_semantic_skipped_requests;
    timings.resolution_import_semantic_requests = telemetry.resolution_import_semantic_requests;
    timings.resolution_import_semantic_unique_requests =
        telemetry.resolution_import_semantic_unique_requests;
    timings.resolution_import_semantic_skipped_requests =
        telemetry.resolution_import_semantic_skipped_requests;
    timings.resolution_call_compute_ms = telemetry.resolution_call_compute_ms;
    timings.resolution_import_compute_ms = telemetry.resolution_import_compute_ms;
    timings.resolution_call_apply_ms = telemetry.resolution_call_apply_ms;
    timings.resolution_import_apply_ms = telemetry.resolution_import_apply_ms;
    timings.resolution_override_resolution_ms = telemetry.resolution_override_resolution_ms;
    timings.resolved_calls_same_file = telemetry.resolved_calls_same_file;
    timings.resolved_calls_same_module = telemetry.resolved_calls_same_module;
    timings.resolved_calls_global_unique = telemetry.resolved_calls_global_unique;
    timings.resolved_calls_semantic = telemetry.resolved_calls_semantic;
    timings.resolved_imports_same_file = telemetry.resolved_imports_same_file;
    timings.resolved_imports_same_module = telemetry.resolved_imports_same_module;
    timings.resolved_imports_global_unique = telemetry.resolved_imports_global_unique;
    timings.resolved_imports_fuzzy = telemetry.resolved_imports_fuzzy;
    timings.resolved_imports_semantic = telemetry.resolved_imports_semantic;
}

fn apply_staged_publication_timings(
    timings: &mut IndexingPhaseTimings,
    finalize_stats: StagedSnapshotFinalizeStats,
    detail_snapshot_ms: u32,
    publish_stats: StagedSnapshotPublishStats,
    publish_duration: Duration,
    semantic_context_index_ms: u32,
) {
    timings.deferred_indexes_ms = Some(
        finalize_stats
            .deferred_indexes_ms
            .saturating_add(semantic_context_index_ms),
    );
    timings.summary_snapshot_ms = Some(finalize_stats.summary_snapshot_ms);
    timings.detail_snapshot_ms = Some(detail_snapshot_ms);
    timings.publish_ms = Some(clamp_u128_to_u32(publish_duration.as_millis()));
    timings.staged_sqlite_wal_autocheckpoint_bytes = publish_stats.sqlite_wal_autocheckpoint_bytes;
    timings.staged_sqlite_checkpoint_ms = publish_stats.sqlite_checkpoint_ms;
    timings.staged_sqlite_sync_ms = publish_stats.sqlite_sync_ms;
    timings.staged_snapshot_copy = publish_stats
        .snapshot_copy
        .map(database_snapshot_copy_timings);
    timings.core_promotion = Some(core_promotion_timings(publish_stats.core_promotion));
}

pub(super) fn apply_full_refresh_pipeline_timings(
    timings: &mut IndexingPhaseTimings,
    stats: &IncrementalIndexingStats,
    wall: FullRefreshWallTimings,
) {
    timings.full_refresh_wall = Some(wall);
    let pipeline_enabled = stats.full_refresh_queue_capacity > 0;
    timings.full_refresh_chunks_produced =
        pipeline_enabled.then_some(clamp_usize_to_u32(stats.full_refresh_chunks_produced));
    timings.full_refresh_chunks_persisted =
        pipeline_enabled.then_some(clamp_usize_to_u32(stats.full_refresh_chunks_persisted));
    timings.full_refresh_queue_capacity =
        pipeline_enabled.then_some(clamp_usize_to_u32(stats.full_refresh_queue_capacity));
    timings.full_refresh_queue_high_water =
        pipeline_enabled.then_some(clamp_usize_to_u32(stats.full_refresh_queue_high_water));
    timings.full_refresh_producer_blocked_ms =
        pipeline_enabled.then_some(clamp_u64_to_u32(stats.full_refresh_producer_blocked_ms));
    timings.full_refresh_writer_idle_ms =
        pipeline_enabled.then_some(clamp_u64_to_u32(stats.full_refresh_writer_idle_ms));
    let chunking_enabled = stats.full_refresh_chunk_target_bytes > 0;
    timings.full_refresh_chunk_target_bytes =
        chunking_enabled.then_some(stats.full_refresh_chunk_target_bytes);
    timings.full_refresh_chunk_target_nodes =
        chunking_enabled.then_some(clamp_usize_to_u32(stats.full_refresh_chunk_target_nodes));
    timings.full_refresh_chunk_file_ceiling =
        chunking_enabled.then_some(clamp_usize_to_u32(stats.full_refresh_chunk_file_ceiling));
    timings.full_refresh_chunk_max_files =
        chunking_enabled.then_some(clamp_usize_to_u32(stats.full_refresh_chunk_max_files));
    timings.full_refresh_chunk_max_planned_bytes =
        chunking_enabled.then_some(stats.full_refresh_chunk_max_planned_bytes);
    timings.full_refresh_chunk_max_nodes =
        chunking_enabled.then_some(clamp_usize_to_u32(stats.full_refresh_chunk_max_nodes));
    timings.full_refresh_chunk_budget_overruns =
        chunking_enabled.then_some(clamp_usize_to_u32(stats.full_refresh_chunk_budget_overruns));
    timings.full_refresh_chunk_planning_ms =
        chunking_enabled.then_some(clamp_u64_to_u32(stats.full_refresh_chunk_planning_ms));
}

pub(super) fn core_indexing_phase_timings(
    stats: &IncrementalIndexingStats,
    finalize_stats: StagedSnapshotFinalizeStats,
    detail_snapshot_ms: u32,
    publish_stats: StagedSnapshotPublishStats,
    publish_duration: Duration,
    semantic_context_index_ms: u32,
) -> IndexingPhaseTimings {
    let mut timings = IndexingPhaseTimings::default();
    apply_core_indexing_stats(&mut timings, stats);
    apply_resolution_telemetry(
        &mut timings,
        &OptionalResolutionTelemetry::from_incremental_stats(stats),
    );
    apply_staged_publication_timings(
        &mut timings,
        finalize_stats,
        detail_snapshot_ms,
        publish_stats,
        publish_duration,
        semantic_context_index_ms,
    );
    timings
}

#[derive(Debug, Clone, Default)]
struct OptionalResolutionTelemetry {
    setup_existing_projection_ids_ms: Option<u32>,
    setup_seed_symbol_table_ms: Option<u32>,
    flush_files_ms: Option<u32>,
    flush_nodes_ms: Option<u32>,
    flush_edges_ms: Option<u32>,
    flush_occurrences_ms: Option<u32>,
    flush_component_access_ms: Option<u32>,
    flush_callable_projection_ms: Option<u32>,
    resolution_override_count_ms: Option<u32>,
    resolution_unresolved_counts_ms: Option<u32>,
    resolution_calls_ms: Option<u32>,
    resolution_imports_ms: Option<u32>,
    resolution_cleanup_ms: Option<u32>,
    resolution_call_candidate_index_ms: Option<u32>,
    resolution_import_candidate_index_ms: Option<u32>,
    resolution_call_semantic_index_ms: Option<u32>,
    resolution_import_semantic_index_ms: Option<u32>,
    resolution_support_snapshot_limit_bytes: Option<u64>,
    resolution_support_snapshot_stored: Option<bool>,
    resolution_support_snapshot_skipped_oversize: Option<bool>,
    resolution_call_semantic_candidates_ms: Option<u32>,
    resolution_import_semantic_candidates_ms: Option<u32>,
    resolution_call_semantic_requests: Option<u32>,
    resolution_call_semantic_unique_requests: Option<u32>,
    resolution_call_semantic_skipped_requests: Option<u32>,
    resolution_import_semantic_requests: Option<u32>,
    resolution_import_semantic_unique_requests: Option<u32>,
    resolution_import_semantic_skipped_requests: Option<u32>,
    resolution_call_compute_ms: Option<u32>,
    resolution_import_compute_ms: Option<u32>,
    resolution_call_apply_ms: Option<u32>,
    resolution_import_apply_ms: Option<u32>,
    resolution_override_resolution_ms: Option<u32>,
    resolved_calls_same_file: Option<u32>,
    resolved_calls_same_module: Option<u32>,
    resolved_calls_global_unique: Option<u32>,
    resolved_calls_semantic: Option<u32>,
    resolved_imports_same_file: Option<u32>,
    resolved_imports_same_module: Option<u32>,
    resolved_imports_global_unique: Option<u32>,
    resolved_imports_fuzzy: Option<u32>,
    resolved_imports_semantic: Option<u32>,
}

fn artifact_cache_access_timings(stats: &ArtifactCacheFamilyStats) -> ArtifactCacheAccessTimings {
    ArtifactCacheAccessTimings {
        policy: match stats.policy {
            ArtifactCachePolicy::KnownEmpty => ArtifactCachePolicyDto::KnownEmpty,
            ArtifactCachePolicy::ReadThrough => ArtifactCachePolicyDto::ReadThrough,
        },
        logical_lookups: clamp_usize_to_u32(stats.logical_lookups),
        physical_queries: clamp_usize_to_u32(stats.physical_queries),
        hits: clamp_usize_to_u32(stats.hits),
        misses: clamp_usize_to_u32(stats.misses),
        reader_opens: clamp_usize_to_u32(stats.reader_opens),
        lookup_wall_ms: clamp_u64_to_u32(stats.lookup_wall_ns / 1_000_000),
    }
}

fn projection_persistence_family_timings(
    stats: codestory_store::ProjectionPersistenceFamilyStats,
) -> ProjectionPersistenceFamilyTimings {
    ProjectionPersistenceFamilyTimings {
        row_attempts: stats.row_attempts,
        bound_bytes: stats.bound_bytes,
        statement_executions: stats.statement_executions,
        wall_ms: stats.wall_ms,
    }
}

fn projection_persistence_timings(
    stats: &codestory_store::ProjectionPersistenceStats,
) -> ProjectionPersistenceTimings {
    ProjectionPersistenceTimings {
        transactions: stats.transactions.min(u32::MAX as u64) as u32,
        row_attempts: stats.row_attempts(),
        bound_bytes: stats.bound_bytes(),
        statement_executions: stats.statement_executions(),
        transaction_wall_ms: stats.transaction_wall_ms,
        transaction_setup_ms: stats.transaction_setup_ms,
        commit_ms: stats.commit_ms,
        files: projection_persistence_family_timings(stats.files),
        nodes: projection_persistence_family_timings(stats.nodes),
        structural_text: projection_persistence_family_timings(stats.structural_text),
        edges: projection_persistence_family_timings(stats.edges),
        occurrences: projection_persistence_family_timings(stats.occurrences),
        component_access: projection_persistence_family_timings(stats.component_access),
        callable_projection: projection_persistence_family_timings(stats.callable_projection),
        file_errors: projection_persistence_family_timings(stats.file_errors),
        dirty_state: projection_persistence_family_timings(stats.dirty_state),
    }
}

pub(super) fn database_snapshot_copy_timings(
    stats: codestory_store::DatabaseSnapshotCopyStats,
) -> DatabaseSnapshotCopyTimings {
    DatabaseSnapshotCopyTimings {
        copy_ms: stats.copy_ms,
        source_bytes: stats.source_bytes,
        target_bytes: stats.target_bytes,
    }
}

pub(super) fn core_promotion_timings(
    stats: codestory_store::CorePromotionStats,
) -> CorePromotionTimings {
    CorePromotionTimings {
        total_ms: stats.total_ms,
        lock_recovery_ms: stats.lock_recovery_ms,
        candidate_validation_ms: stats.candidate_validation_ms,
        previous_validation_ms: stats.previous_validation_ms,
        rollback_backup_copy_ms: stats.rollback_backup_copy_ms,
        backup_validation_ms: stats.backup_validation_ms,
        prepared_journal_write_ms: stats.prepared_journal_write_ms,
        prepared_journal_file_sync_ms: stats.prepared_journal_file_sync_ms,
        prepared_journal_directory_sync_ms: stats.prepared_journal_directory_sync_ms,
        staged_to_live_restore_ms: stats.staged_to_live_restore_ms,
        promoted_validation_ms: stats.promoted_validation_ms,
        committed_journal_ms: stats.committed_journal_ms,
        cleanup_ms: stats.cleanup_ms,
        unattributed_ms: stats.unattributed_ms,
        candidate_bytes: stats.candidate_bytes,
        previous_live_bytes: stats.previous_live_bytes,
        rollback_backup_bytes: stats.rollback_backup_bytes,
    }
}

impl OptionalResolutionTelemetry {
    fn from_incremental_stats(index_stats: &IncrementalIndexingStats) -> Self {
        let mut telemetry = Self::from_flush_stats(index_stats);
        if index_stats.resolution_ran {
            telemetry.apply_resolution_stats(index_stats);
        }
        telemetry
    }

    fn from_flush_stats(index_stats: &IncrementalIndexingStats) -> Self {
        Self {
            setup_existing_projection_ids_ms: Some(clamp_u64_to_u32(
                index_stats.setup_existing_projection_ids_ms,
            )),
            setup_seed_symbol_table_ms: Some(clamp_u64_to_u32(
                index_stats.setup_seed_symbol_table_ms,
            )),
            flush_files_ms: Some(clamp_u64_to_u32(index_stats.flush_files_ms)),
            flush_nodes_ms: Some(clamp_u64_to_u32(index_stats.flush_nodes_ms)),
            flush_edges_ms: Some(clamp_u64_to_u32(index_stats.flush_edges_ms)),
            flush_occurrences_ms: Some(clamp_u64_to_u32(index_stats.flush_occurrences_ms)),
            flush_component_access_ms: Some(clamp_u64_to_u32(
                index_stats.flush_component_access_ms,
            )),
            flush_callable_projection_ms: Some(clamp_u64_to_u32(
                index_stats.flush_callable_projection_ms,
            )),
            ..Self::default()
        }
    }

    fn apply_resolution_stats(&mut self, index_stats: &IncrementalIndexingStats) {
        self.resolution_override_count_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_override_count_ms));
        self.resolution_unresolved_counts_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_unresolved_counts_ms,
        ));
        self.resolution_calls_ms = Some(clamp_u64_to_u32(index_stats.resolution_calls_ms));
        self.resolution_imports_ms = Some(clamp_u64_to_u32(index_stats.resolution_imports_ms));
        self.resolution_cleanup_ms = Some(clamp_u64_to_u32(index_stats.resolution_cleanup_ms));
        self.resolution_call_candidate_index_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_call_candidate_index_ms,
        ));
        self.resolution_import_candidate_index_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_import_candidate_index_ms,
        ));
        self.resolution_call_semantic_index_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_call_semantic_index_ms,
        ));
        self.resolution_import_semantic_index_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_import_semantic_index_ms,
        ));
        self.resolution_support_snapshot_limit_bytes =
            Some(index_stats.resolution_support_snapshot_limit_bytes);
        self.resolution_support_snapshot_stored =
            Some(index_stats.resolution_support_snapshot_stored);
        self.resolution_support_snapshot_skipped_oversize =
            Some(index_stats.resolution_support_snapshot_skipped_oversize);
        self.resolution_call_semantic_candidates_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_call_semantic_candidates_ms,
        ));
        self.resolution_import_semantic_candidates_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_import_semantic_candidates_ms,
        ));
        self.resolution_call_semantic_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_call_semantic_requests,
        ));
        self.resolution_call_semantic_unique_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_call_semantic_unique_requests,
        ));
        self.resolution_call_semantic_skipped_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_call_semantic_skipped_requests,
        ));
        self.resolution_import_semantic_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_import_semantic_requests,
        ));
        self.resolution_import_semantic_unique_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_import_semantic_unique_requests,
        ));
        self.resolution_import_semantic_skipped_requests = Some(clamp_usize_to_u32(
            index_stats.resolution_import_semantic_skipped_requests,
        ));
        self.resolution_call_compute_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_call_compute_ms));
        self.resolution_import_compute_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_import_compute_ms));
        self.resolution_call_apply_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_call_apply_ms));
        self.resolution_import_apply_ms =
            Some(clamp_u64_to_u32(index_stats.resolution_import_apply_ms));
        self.resolution_override_resolution_ms = Some(clamp_u64_to_u32(
            index_stats.resolution_override_resolution_ms,
        ));
        self.resolved_calls_same_file =
            Some(clamp_usize_to_u32(index_stats.resolved_calls_same_file));
        self.resolved_calls_same_module =
            Some(clamp_usize_to_u32(index_stats.resolved_calls_same_module));
        self.resolved_calls_global_unique =
            Some(clamp_usize_to_u32(index_stats.resolved_calls_global_unique));
        self.resolved_calls_semantic =
            Some(clamp_usize_to_u32(index_stats.resolved_calls_semantic));
        self.resolved_imports_same_file =
            Some(clamp_usize_to_u32(index_stats.resolved_imports_same_file));
        self.resolved_imports_same_module =
            Some(clamp_usize_to_u32(index_stats.resolved_imports_same_module));
        self.resolved_imports_global_unique = Some(clamp_usize_to_u32(
            index_stats.resolved_imports_global_unique,
        ));
        self.resolved_imports_fuzzy = Some(clamp_usize_to_u32(index_stats.resolved_imports_fuzzy));
        self.resolved_imports_semantic =
            Some(clamp_usize_to_u32(index_stats.resolved_imports_semantic));
    }
}
