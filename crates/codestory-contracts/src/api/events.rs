use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FullRefreshWallTimings {
    pub core_refresh_ms: u32,
    pub live_inspection_ms: u32,
    pub source_discovery_ms: u32,
    pub stage_open_ms: u32,
    pub indexer_execution_ms: u32,
    pub coverage_validation_ms: u32,
    pub copy_forward_ms: u32,
    pub semantic_stage_ms: u32,
    pub snapshot_stage_ms: u32,
    pub publication_prepare_ms: u32,
    pub search_generation_ms: u32,
    pub catalog_publication_ms: u32,
    pub unattributed_ms: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DatabaseSnapshotCopyTimings {
    pub copy_ms: u32,
    pub source_bytes: u64,
    pub target_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CorePromotionTimings {
    pub total_ms: u32,
    pub lock_recovery_ms: u32,
    pub candidate_validation_ms: u32,
    pub previous_validation_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_backup_copy_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_validation_ms: Option<u32>,
    pub prepared_journal_write_ms: u32,
    pub prepared_journal_file_sync_ms: u32,
    pub prepared_journal_directory_sync_ms: u32,
    pub staged_to_live_restore_ms: u32,
    pub promoted_validation_ms: u32,
    pub committed_journal_ms: u32,
    pub cleanup_ms: u32,
    pub unattributed_ms: u32,
    pub candidate_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_live_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_backup_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactCachePolicyDto {
    KnownEmpty,
    #[default]
    ReadThrough,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ArtifactCacheAccessTimings {
    pub policy: ArtifactCachePolicyDto,
    pub logical_lookups: u32,
    pub physical_queries: u32,
    pub hits: u32,
    pub misses: u32,
    pub reader_opens: u32,
    pub lookup_wall_ms: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectionPersistenceFamilyTimings {
    pub row_attempts: u64,
    pub bound_bytes: u64,
    pub statement_executions: u64,
    pub wall_ms: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectionPersistenceTimings {
    pub transactions: u32,
    pub row_attempts: u64,
    pub bound_bytes: u64,
    pub statement_executions: u64,
    pub transaction_wall_ms: u32,
    pub transaction_setup_ms: u32,
    pub commit_ms: u32,
    pub files: ProjectionPersistenceFamilyTimings,
    pub nodes: ProjectionPersistenceFamilyTimings,
    pub structural_text: ProjectionPersistenceFamilyTimings,
    pub edges: ProjectionPersistenceFamilyTimings,
    pub occurrences: ProjectionPersistenceFamilyTimings,
    pub component_access: ProjectionPersistenceFamilyTimings,
    pub callable_projection: ProjectionPersistenceFamilyTimings,
    pub file_errors: ProjectionPersistenceFamilyTimings,
    pub dirty_state: ProjectionPersistenceFamilyTimings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct IndexingPhaseTimings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_wall: Option<FullRefreshWallTimings>,
    pub parse_index_ms: u32,
    pub projection_flush_ms: u32,
    pub edge_resolution_ms: u32,
    pub error_flush_ms: u32,
    pub cleanup_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_cache_write_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_cache_writes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_cache_write_transactions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parser_artifact_cache: Option<ArtifactCacheAccessTimings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structural_artifact_cache: Option<ArtifactCacheAccessTimings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_chunks_produced: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_chunks_persisted: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_queue_capacity: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_queue_high_water: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_producer_blocked_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_writer_idle_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_chunk_target_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_chunk_target_nodes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_chunk_file_ceiling: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_chunk_max_files: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_chunk_max_planned_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_chunk_max_nodes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_chunk_budget_overruns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_refresh_chunk_planning_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_prepare_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_batch_wall_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_batch_transactions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_persistence: Option<ProjectionPersistenceTimings>,
    pub cache_refresh_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_projection_rebuild_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_symbol_stream_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_symbol_stream_rows: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_symbol_stream_batches: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_symbol_index_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_symbol_index_docs_written: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_symbol_index_writer_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_symbol_index_commit_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_symbol_index_reload_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_symbol_index_commit_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_symbol_index_reload_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_cache_publish_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_context_index_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_node_load_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_node_load_rows: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_node_stream_batches: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_endpoint_load_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_endpoint_load_rows: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_endpoint_load_batches: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_selected_nodes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_context_file_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_context_path_bytes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_node_lookup_entries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_context_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_doc_build_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_embedding_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_db_upsert_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_reload_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_prune_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_docs_reused: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_docs_embedded: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_docs_pending: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_docs_stale: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_search_docs_written: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_dense_docs_skipped: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_dense_public_api: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_dense_entrypoint: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_dense_documented_nontrivial: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_dense_central_graph_node: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_dense_component_report: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_dense_unstructured_doc: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_indexes_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_snapshot_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_snapshot_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_sqlite_wal_autocheckpoint_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_sqlite_checkpoint_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_sqlite_sync_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_snapshot_copy: Option<DatabaseSnapshotCopyTimings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_promotion: Option<CorePromotionTimings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_existing_projection_ids_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_seed_symbol_table_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flush_files_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flush_nodes_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flush_edges_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flush_occurrences_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flush_component_access_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flush_callable_projection_ms: Option<u32>,
    pub unresolved_calls_start: u32,
    pub unresolved_imports_start: u32,
    pub resolved_calls: u32,
    pub resolved_imports: u32,
    pub unresolved_calls_end: u32,
    pub unresolved_imports_end: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_override_count_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_unresolved_counts_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_calls_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_imports_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_cleanup_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_call_candidate_index_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_import_candidate_index_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_call_semantic_index_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_import_semantic_index_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_support_snapshot_limit_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_support_snapshot_stored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_support_snapshot_skipped_oversize: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_call_semantic_candidates_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_import_semantic_candidates_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_call_semantic_requests: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_call_semantic_unique_requests: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_call_semantic_skipped_requests: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_import_semantic_requests: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_import_semantic_unique_requests: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_import_semantic_skipped_requests: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_call_compute_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_import_compute_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_call_apply_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_import_apply_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_override_resolution_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_calls_same_file: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_calls_same_module: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_calls_global_unique: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_calls_semantic: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_imports_same_file: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_imports_same_module: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_imports_global_unique: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_imports_fuzzy: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_imports_semantic: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "type", content = "data")]
#[allow(clippy::large_enum_variant)]
pub enum AppEventPayload {
    // Use u32 so TS can safely represent these as `number` without BigInt.
    IndexingStarted {
        file_count: u32,
    },
    IndexingProgress {
        current: u32,
        total: u32,
    },
    IndexingComplete {
        duration_ms: u32,
        phase_timings: IndexingPhaseTimings,
    },
    IndexingFailed {
        error: String,
    },
    StatusUpdate {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_payload_serializes_with_type_and_data() {
        let ev = AppEventPayload::IndexingStarted { file_count: 3 };
        let v = serde_json::to_value(ev).expect("serialize");
        assert_eq!(v["type"], "IndexingStarted");
        assert_eq!(v["data"]["file_count"], 3);
    }

    #[test]
    fn test_indexing_phase_timings_omits_optional_resolution_fields_when_none() {
        let timings = IndexingPhaseTimings {
            full_refresh_wall: None,
            parse_index_ms: 1,
            projection_flush_ms: 2,
            edge_resolution_ms: 3,
            error_flush_ms: 4,
            cleanup_ms: 5,
            artifact_cache_write_ms: None,
            artifact_cache_writes: None,
            artifact_cache_write_transactions: None,
            parser_artifact_cache: None,
            structural_artifact_cache: None,
            full_refresh_chunks_produced: None,
            full_refresh_chunks_persisted: None,
            full_refresh_queue_capacity: None,
            full_refresh_queue_high_water: None,
            full_refresh_producer_blocked_ms: None,
            full_refresh_writer_idle_ms: None,
            full_refresh_chunk_target_bytes: None,
            full_refresh_chunk_target_nodes: None,
            full_refresh_chunk_file_ceiling: None,
            full_refresh_chunk_max_files: None,
            full_refresh_chunk_max_planned_bytes: None,
            full_refresh_chunk_max_nodes: None,
            full_refresh_chunk_budget_overruns: None,
            full_refresh_chunk_planning_ms: None,
            source_prepare_ms: None,
            projection_batch_wall_ms: None,
            projection_batch_transactions: None,
            projection_persistence: None,
            cache_refresh_ms: None,
            search_projection_rebuild_ms: None,
            search_symbol_stream_ms: None,
            search_symbol_stream_rows: None,
            search_symbol_stream_batches: None,
            search_symbol_index_ms: None,
            search_symbol_index_docs_written: None,
            search_symbol_index_writer_count: None,
            search_symbol_index_commit_count: None,
            search_symbol_index_reload_count: None,
            search_symbol_index_commit_ms: None,
            search_symbol_index_reload_ms: None,
            runtime_cache_publish_ms: None,
            semantic_context_index_ms: None,
            semantic_node_load_ms: None,
            semantic_node_load_rows: None,
            semantic_node_stream_batches: None,
            semantic_endpoint_load_ms: None,
            semantic_endpoint_load_rows: None,
            semantic_endpoint_load_batches: None,
            semantic_selected_nodes: None,
            semantic_context_file_count: None,
            semantic_context_path_bytes: None,
            semantic_node_lookup_entries: None,
            semantic_context_ms: None,
            semantic_doc_build_ms: None,
            semantic_embedding_ms: None,
            semantic_db_upsert_ms: None,
            semantic_reload_ms: None,
            semantic_prune_ms: None,
            semantic_docs_reused: None,
            semantic_docs_embedded: None,
            semantic_docs_pending: None,
            semantic_docs_stale: None,
            symbol_search_docs_written: None,
            semantic_dense_docs_skipped: None,
            semantic_dense_public_api: None,
            semantic_dense_entrypoint: None,
            semantic_dense_documented_nontrivial: None,
            semantic_dense_central_graph_node: None,
            semantic_dense_component_report: None,
            semantic_dense_unstructured_doc: None,
            deferred_indexes_ms: None,
            summary_snapshot_ms: None,
            detail_snapshot_ms: None,
            publish_ms: None,
            staged_sqlite_wal_autocheckpoint_bytes: None,
            staged_sqlite_checkpoint_ms: None,
            staged_sqlite_sync_ms: None,
            staged_snapshot_copy: None,
            core_promotion: None,
            setup_existing_projection_ids_ms: None,
            setup_seed_symbol_table_ms: None,
            flush_files_ms: None,
            flush_nodes_ms: None,
            flush_edges_ms: None,
            flush_occurrences_ms: None,
            flush_component_access_ms: None,
            flush_callable_projection_ms: None,
            unresolved_calls_start: 6,
            unresolved_imports_start: 7,
            resolved_calls: 8,
            resolved_imports: 9,
            unresolved_calls_end: 10,
            unresolved_imports_end: 11,
            resolution_override_count_ms: None,
            resolution_unresolved_counts_ms: None,
            resolution_calls_ms: None,
            resolution_imports_ms: None,
            resolution_cleanup_ms: None,
            resolution_call_candidate_index_ms: None,
            resolution_import_candidate_index_ms: None,
            resolution_call_semantic_index_ms: None,
            resolution_import_semantic_index_ms: None,
            resolution_support_snapshot_limit_bytes: None,
            resolution_support_snapshot_stored: None,
            resolution_support_snapshot_skipped_oversize: None,
            resolution_call_semantic_candidates_ms: None,
            resolution_import_semantic_candidates_ms: None,
            resolution_call_semantic_requests: None,
            resolution_call_semantic_unique_requests: None,
            resolution_call_semantic_skipped_requests: None,
            resolution_import_semantic_requests: None,
            resolution_import_semantic_unique_requests: None,
            resolution_import_semantic_skipped_requests: None,
            resolution_call_compute_ms: None,
            resolution_import_compute_ms: None,
            resolution_call_apply_ms: None,
            resolution_import_apply_ms: None,
            resolution_override_resolution_ms: None,
            resolved_calls_same_file: None,
            resolved_calls_same_module: None,
            resolved_calls_global_unique: None,
            resolved_calls_semantic: None,
            resolved_imports_same_file: None,
            resolved_imports_same_module: None,
            resolved_imports_global_unique: None,
            resolved_imports_fuzzy: None,
            resolved_imports_semantic: None,
        };

        let value = serde_json::to_value(timings).expect("serialize timings");
        assert!(value.get("full_refresh_wall").is_none());
        assert!(value.get("artifact_cache_write_ms").is_none());
        assert!(value.get("artifact_cache_writes").is_none());
        assert!(value.get("artifact_cache_write_transactions").is_none());
        assert!(value.get("full_refresh_chunks_produced").is_none());
        assert!(value.get("full_refresh_chunks_persisted").is_none());
        assert!(value.get("full_refresh_queue_capacity").is_none());
        assert!(value.get("full_refresh_queue_high_water").is_none());
        assert!(value.get("full_refresh_producer_blocked_ms").is_none());
        assert!(value.get("full_refresh_writer_idle_ms").is_none());
        assert!(value.get("full_refresh_chunk_target_bytes").is_none());
        assert!(value.get("full_refresh_chunk_target_nodes").is_none());
        assert!(value.get("full_refresh_chunk_file_ceiling").is_none());
        assert!(value.get("full_refresh_chunk_max_files").is_none());
        assert!(value.get("full_refresh_chunk_max_planned_bytes").is_none());
        assert!(value.get("full_refresh_chunk_max_nodes").is_none());
        assert!(value.get("full_refresh_chunk_budget_overruns").is_none());
        assert!(value.get("full_refresh_chunk_planning_ms").is_none());
        assert!(value.get("source_prepare_ms").is_none());
        assert!(value.get("projection_batch_wall_ms").is_none());
        assert!(value.get("projection_batch_transactions").is_none());
        assert!(value.get("projection_persistence").is_none());
        assert!(value.get("resolution_unresolved_counts_ms").is_none());
        assert!(value.get("resolution_calls_ms").is_none());
        assert!(value.get("resolution_imports_ms").is_none());
        assert!(value.get("resolution_cleanup_ms").is_none());
        assert!(value.get("semantic_context_index_ms").is_none());
        assert!(value.get("semantic_doc_build_ms").is_none());
        assert!(value.get("semantic_node_load_ms").is_none());
        assert!(value.get("semantic_node_load_rows").is_none());
        assert!(value.get("semantic_node_stream_batches").is_none());
        assert!(value.get("semantic_endpoint_load_ms").is_none());
        assert!(value.get("semantic_endpoint_load_rows").is_none());
        assert!(value.get("semantic_endpoint_load_batches").is_none());
        assert!(value.get("semantic_selected_nodes").is_none());
        assert!(value.get("semantic_context_file_count").is_none());
        assert!(value.get("semantic_context_path_bytes").is_none());
        assert!(value.get("semantic_node_lookup_entries").is_none());
        assert!(value.get("semantic_context_ms").is_none());
        assert!(value.get("semantic_embedding_ms").is_none());
        assert!(value.get("semantic_db_upsert_ms").is_none());
        assert!(value.get("semantic_reload_ms").is_none());
        assert!(value.get("semantic_prune_ms").is_none());
        assert!(value.get("search_projection_rebuild_ms").is_none());
        assert!(value.get("search_symbol_stream_ms").is_none());
        assert!(value.get("search_symbol_stream_rows").is_none());
        assert!(value.get("search_symbol_stream_batches").is_none());
        assert!(value.get("search_symbol_index_ms").is_none());
        assert!(value.get("search_symbol_index_docs_written").is_none());
        assert!(value.get("search_symbol_index_writer_count").is_none());
        assert!(value.get("search_symbol_index_commit_count").is_none());
        assert!(value.get("search_symbol_index_reload_count").is_none());
        assert!(value.get("search_symbol_index_commit_ms").is_none());
        assert!(value.get("search_symbol_index_reload_ms").is_none());
        assert!(value.get("runtime_cache_publish_ms").is_none());
        assert!(value.get("semantic_docs_reused").is_none());
        assert!(value.get("semantic_docs_embedded").is_none());
        assert!(value.get("semantic_docs_pending").is_none());
        assert!(value.get("semantic_docs_stale").is_none());
        assert!(value.get("symbol_search_docs_written").is_none());
        assert!(value.get("semantic_dense_docs_skipped").is_none());
        assert!(value.get("semantic_dense_public_api").is_none());
        assert!(value.get("semantic_dense_entrypoint").is_none());
        assert!(value.get("semantic_dense_documented_nontrivial").is_none());
        assert!(value.get("semantic_dense_central_graph_node").is_none());
        assert!(value.get("semantic_dense_component_report").is_none());
        assert!(value.get("semantic_dense_unstructured_doc").is_none());
        assert!(value.get("resolution_call_candidate_index_ms").is_none());
        assert!(value.get("resolution_import_candidate_index_ms").is_none());
        assert!(value.get("resolution_call_semantic_index_ms").is_none());
        assert!(value.get("resolution_import_semantic_index_ms").is_none());
        assert!(
            value
                .get("resolution_support_snapshot_limit_bytes")
                .is_none()
        );
        assert!(value.get("resolution_support_snapshot_stored").is_none());
        assert!(
            value
                .get("resolution_support_snapshot_skipped_oversize")
                .is_none()
        );
        assert!(
            value
                .get("resolution_call_semantic_candidates_ms")
                .is_none()
        );
        assert!(
            value
                .get("resolution_import_semantic_candidates_ms")
                .is_none()
        );
        assert!(value.get("resolution_call_compute_ms").is_none());
        assert!(value.get("resolution_import_compute_ms").is_none());
        assert!(value.get("resolution_call_apply_ms").is_none());
        assert!(value.get("resolution_import_apply_ms").is_none());
        assert!(value.get("resolution_override_resolution_ms").is_none());
        assert!(value.get("deferred_indexes_ms").is_none());
        assert!(value.get("summary_snapshot_ms").is_none());
        assert!(value.get("detail_snapshot_ms").is_none());
        assert!(value.get("publish_ms").is_none());
        assert!(
            value
                .get("staged_sqlite_wal_autocheckpoint_bytes")
                .is_none()
        );
        assert!(value.get("staged_sqlite_checkpoint_ms").is_none());
        assert!(value.get("staged_sqlite_sync_ms").is_none());
        assert!(value.get("staged_snapshot_copy").is_none());
        assert!(value.get("core_promotion").is_none());
        assert!(value.get("setup_existing_projection_ids_ms").is_none());
        assert!(value.get("setup_seed_symbol_table_ms").is_none());
        assert!(value.get("flush_files_ms").is_none());
        assert!(value.get("flush_nodes_ms").is_none());
        assert!(value.get("flush_edges_ms").is_none());
        assert!(value.get("flush_occurrences_ms").is_none());
        assert!(value.get("flush_component_access_ms").is_none());
        assert!(value.get("flush_callable_projection_ms").is_none());
        assert!(value.get("resolution_override_count_ms").is_none());
        assert!(value.get("resolved_calls_same_file").is_none());
        assert!(value.get("resolved_calls_same_module").is_none());
        assert!(value.get("resolved_calls_global_unique").is_none());
        assert!(value.get("resolved_calls_semantic").is_none());
        assert!(value.get("resolved_imports_same_file").is_none());
        assert!(value.get("resolved_imports_same_module").is_none());
        assert!(value.get("resolved_imports_global_unique").is_none());
        assert!(value.get("resolved_imports_fuzzy").is_none());
        assert!(value.get("resolved_imports_semantic").is_none());
    }

    #[test]
    fn test_indexing_phase_timings_accepts_legacy_json_without_full_refresh_wall() {
        let timings: IndexingPhaseTimings = serde_json::from_value(serde_json::json!({
            "parse_index_ms": 1,
            "projection_flush_ms": 2,
            "edge_resolution_ms": 3,
            "error_flush_ms": 4,
            "cleanup_ms": 5,
            "cache_refresh_ms": null,
            "unresolved_calls_start": 6,
            "unresolved_imports_start": 7,
            "resolved_calls": 8,
            "resolved_imports": 9,
            "unresolved_calls_end": 10,
            "unresolved_imports_end": 11
        }))
        .expect("deserialize legacy timings");

        assert!(timings.full_refresh_wall.is_none());
        assert!(timings.semantic_context_index_ms.is_none());
        assert!(timings.semantic_node_load_ms.is_none());
        assert!(timings.search_symbol_index_commit_ms.is_none());
        assert!(timings.parser_artifact_cache.is_none());
        assert!(timings.structural_artifact_cache.is_none());
        assert!(timings.staged_snapshot_copy.is_none());
        assert!(timings.core_promotion.is_none());
    }

    #[test]
    fn test_indexing_phase_timings_round_trips_separate_artifact_cache_families() {
        let timings = IndexingPhaseTimings {
            parser_artifact_cache: Some(ArtifactCacheAccessTimings {
                policy: ArtifactCachePolicyDto::KnownEmpty,
                logical_lookups: 3,
                physical_queries: 0,
                hits: 0,
                misses: 3,
                reader_opens: 0,
                lookup_wall_ms: 0,
            }),
            structural_artifact_cache: Some(ArtifactCacheAccessTimings {
                policy: ArtifactCachePolicyDto::ReadThrough,
                logical_lookups: 2,
                physical_queries: 2,
                hits: 1,
                misses: 1,
                reader_opens: 1,
                lookup_wall_ms: 4,
            }),
            ..IndexingPhaseTimings::default()
        };

        let value = serde_json::to_value(&timings).expect("serialize timings");
        assert_eq!(value["parser_artifact_cache"]["policy"], "known_empty");
        assert_eq!(value["structural_artifact_cache"]["policy"], "read_through");
        let decoded: IndexingPhaseTimings =
            serde_json::from_value(value).expect("deserialize timings");
        assert_eq!(decoded.parser_artifact_cache, timings.parser_artifact_cache);
        assert_eq!(
            decoded.structural_artifact_cache,
            timings.structural_artifact_cache
        );
    }

    #[test]
    fn test_indexing_phase_timings_round_trips_projection_persistence_shape() {
        let persistence = ProjectionPersistenceTimings {
            transactions: 2,
            row_attempts: 20,
            bound_bytes: 2_048,
            statement_executions: 18,
            transaction_wall_ms: 12,
            transaction_setup_ms: 1,
            commit_ms: 3,
            files: ProjectionPersistenceFamilyTimings {
                row_attempts: 2,
                bound_bytes: 256,
                statement_executions: 2,
                wall_ms: 1,
            },
            dirty_state: ProjectionPersistenceFamilyTimings {
                row_attempts: 8,
                bound_bytes: 96,
                statement_executions: 8,
                wall_ms: 2,
            },
            ..ProjectionPersistenceTimings::default()
        };
        let timings = IndexingPhaseTimings {
            projection_persistence: Some(persistence.clone()),
            ..IndexingPhaseTimings::default()
        };

        let value = serde_json::to_value(&timings).expect("serialize timings");
        assert_eq!(value["projection_persistence"]["transactions"], 2);
        assert_eq!(
            value["projection_persistence"]["dirty_state"]["statement_executions"],
            8
        );
        let decoded: IndexingPhaseTimings =
            serde_json::from_value(value).expect("deserialize timings");
        assert_eq!(decoded.projection_persistence, Some(persistence));
    }

    #[test]
    fn test_indexing_phase_timings_round_trips_full_refresh_wall() {
        let wall = FullRefreshWallTimings {
            core_refresh_ms: 78,
            live_inspection_ms: 1,
            source_discovery_ms: 2,
            stage_open_ms: 3,
            indexer_execution_ms: 4,
            coverage_validation_ms: 5,
            copy_forward_ms: 6,
            semantic_stage_ms: 7,
            snapshot_stage_ms: 8,
            publication_prepare_ms: 9,
            search_generation_ms: 10,
            catalog_publication_ms: 11,
            unattributed_ms: 12,
        };
        let timings = IndexingPhaseTimings {
            full_refresh_wall: Some(wall.clone()),
            ..IndexingPhaseTimings::default()
        };

        let value = serde_json::to_value(&timings).expect("serialize timings");
        assert_eq!(value["full_refresh_wall"]["core_refresh_ms"], 78);
        let decoded: IndexingPhaseTimings =
            serde_json::from_value(value).expect("deserialize timings");
        assert_eq!(decoded.full_refresh_wall, Some(wall));
    }

    #[test]
    fn test_indexing_phase_timings_round_trips_core_promotion_diagnostics() {
        let snapshot_copy = DatabaseSnapshotCopyTimings {
            copy_ms: 13,
            source_bytes: 1_024,
            target_bytes: 1_024,
        };
        let core_promotion = CorePromotionTimings {
            total_ms: 89,
            lock_recovery_ms: 1,
            candidate_validation_ms: 11,
            previous_validation_ms: 12,
            rollback_backup_copy_ms: Some(13),
            backup_validation_ms: Some(14),
            prepared_journal_write_ms: 2,
            prepared_journal_file_sync_ms: 3,
            prepared_journal_directory_sync_ms: 4,
            staged_to_live_restore_ms: 15,
            promoted_validation_ms: 10,
            committed_journal_ms: 2,
            cleanup_ms: 1,
            unattributed_ms: 1,
            candidate_bytes: 2_048,
            previous_live_bytes: Some(1_024),
            rollback_backup_bytes: Some(1_024),
        };
        let timings = IndexingPhaseTimings {
            staged_snapshot_copy: Some(snapshot_copy.clone()),
            core_promotion: Some(core_promotion.clone()),
            ..IndexingPhaseTimings::default()
        };

        let value = serde_json::to_value(&timings).expect("serialize timings");
        assert_eq!(value["staged_snapshot_copy"]["copy_ms"], 13);
        assert_eq!(value["core_promotion"]["staged_to_live_restore_ms"], 15);
        let decoded: IndexingPhaseTimings =
            serde_json::from_value(value).expect("deserialize timings");
        assert_eq!(decoded.staged_snapshot_copy, Some(snapshot_copy));
        assert_eq!(decoded.core_promotion, Some(core_promotion));
    }
}
