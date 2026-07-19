use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct IndexingPhaseTimings {
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
    pub cache_refresh_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_projection_rebuild_ms: Option<u32>,
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
    pub runtime_cache_publish_ms: Option<u32>,
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
            parse_index_ms: 1,
            projection_flush_ms: 2,
            edge_resolution_ms: 3,
            error_flush_ms: 4,
            cleanup_ms: 5,
            artifact_cache_write_ms: None,
            artifact_cache_writes: None,
            artifact_cache_write_transactions: None,
            full_refresh_chunks_produced: None,
            full_refresh_chunks_persisted: None,
            full_refresh_queue_capacity: None,
            full_refresh_queue_high_water: None,
            full_refresh_producer_blocked_ms: None,
            full_refresh_writer_idle_ms: None,
            cache_refresh_ms: None,
            search_projection_rebuild_ms: None,
            search_symbol_index_ms: None,
            search_symbol_index_docs_written: None,
            search_symbol_index_writer_count: None,
            search_symbol_index_commit_count: None,
            search_symbol_index_reload_count: None,
            runtime_cache_publish_ms: None,
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
        assert!(value.get("artifact_cache_write_ms").is_none());
        assert!(value.get("artifact_cache_writes").is_none());
        assert!(value.get("artifact_cache_write_transactions").is_none());
        assert!(value.get("full_refresh_chunks_produced").is_none());
        assert!(value.get("full_refresh_chunks_persisted").is_none());
        assert!(value.get("full_refresh_queue_capacity").is_none());
        assert!(value.get("full_refresh_queue_high_water").is_none());
        assert!(value.get("full_refresh_producer_blocked_ms").is_none());
        assert!(value.get("full_refresh_writer_idle_ms").is_none());
        assert!(value.get("resolution_unresolved_counts_ms").is_none());
        assert!(value.get("resolution_calls_ms").is_none());
        assert!(value.get("resolution_imports_ms").is_none());
        assert!(value.get("resolution_cleanup_ms").is_none());
        assert!(value.get("semantic_doc_build_ms").is_none());
        assert!(value.get("semantic_embedding_ms").is_none());
        assert!(value.get("semantic_db_upsert_ms").is_none());
        assert!(value.get("semantic_reload_ms").is_none());
        assert!(value.get("semantic_prune_ms").is_none());
        assert!(value.get("search_projection_rebuild_ms").is_none());
        assert!(value.get("search_symbol_index_ms").is_none());
        assert!(value.get("search_symbol_index_docs_written").is_none());
        assert!(value.get("search_symbol_index_writer_count").is_none());
        assert!(value.get("search_symbol_index_commit_count").is_none());
        assert!(value.get("search_symbol_index_reload_count").is_none());
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
}
