use super::super::test_support::{sample_phase_timings, sample_retrieval, summary_with_files};
use crate::app::source_commands::append_symbol_workflow_nodes;
use crate::args::{IndexDryRunOutput, IndexOutput};
use crate::output::{render_index_dry_run_markdown, render_index_markdown};
use crate::runtime::fnv1a_hex;
use codestory_contracts::api::{IndexDryRunDto, IndexMode, NodeId};

#[test]
fn symbol_workflow_renderer_keeps_caller_shape() {
    let mut markdown = String::new();
    append_symbol_workflow_nodes(
        &mut markdown,
        "direct_callers",
        &[codestory_runtime::SymbolWorkflowNode {
            node_id: NodeId("caller".to_string()),
            display_name: "Caller".to_string(),
            kind: "function".to_string(),
            file_path: Some("src/lib.rs".to_string()),
            depth: 1,
        }],
    );

    assert_eq!(
        markdown,
        "direct_callers:\n- [caller] Caller (function) depth=1 src/lib.rs\n"
    );
}

#[test]
fn fnv1a_hash_is_stable() {
    assert_eq!(fnv1a_hex(b"abc"), "e71fa2190541574b");
}

#[test]
fn dry_run_reports_requested_effective_and_compatibility_reason() {
    let dry_run = IndexDryRunDto {
        root: "/tmp/project".to_string(),
        storage_path: "/tmp/cache/codestory.db".to_string(),
        refresh: IndexMode::Full,
        files_to_index: 4,
        files_to_remove: 0,
        sample_files_to_index: Vec::new(),
        sample_file_ids_to_remove: Vec::new(),
        members: Vec::new(),
    };
    let output = IndexDryRunOutput {
        requested_refresh: "auto",
        effective_refresh: "full",
        compatibility_reason: Some("structural_publication_incompatible"),
        dry_run: &dry_run,
    };

    let json = serde_json::to_value(&output).expect("serialize dry-run decision");
    assert_eq!(json["requested_refresh"], "auto");
    assert_eq!(json["effective_refresh"], "full");
    assert_eq!(
        json["compatibility_reason"],
        "structural_publication_incompatible"
    );
    let markdown = render_index_dry_run_markdown(&output);
    assert!(markdown.contains("requested_refresh: `auto`"));
    assert!(markdown.contains("effective_refresh: `full`"));
    assert!(markdown.contains("compatibility_reason: `structural_publication_incompatible`"));
}

#[test]
fn render_index_markdown_includes_rich_timing_breakdown_when_available() {
    let summary = summary_with_files(3);
    let timings = sample_phase_timings();
    let retrieval = sample_retrieval();
    let output = IndexOutput {
        project: &summary.root,
        storage_path: "C:/repo/.cache/index.sqlite",
        refresh: "auto(full)",
        refresh_reason: Some("structural_publication_incompatible"),
        summary: &summary,
        retrieval: &retrieval,
        phase_timings: Some(&timings),
        summary_generation: None,
        readiness: Vec::new(),
        next_commands: Vec::new(),
    };

    let markdown = render_index_markdown(&output);

    assert!(markdown.contains("refresh: `auto(full)`"));
    assert!(markdown.contains("refresh_reason: `structural_publication_incompatible`"));

    assert!(markdown.contains(
        "cache_ms: artifact_write=6 search_projection=61 search_index=62 runtime_publish=63"
    ));
    assert!(markdown.contains("artifact_cache: writes=24 transactions=1"));
    assert!(markdown.contains(
            "full_refresh_pipeline: produced=2 persisted=2 queue_capacity=1 queue_high_water=1 producer_blocked_ms=3 writer_idle_ms=4"
        ));
    assert!(markdown.contains(
            "full_refresh_chunking: target_bytes=8388608 target_nodes=120000 file_ceiling=512 max_files=384 max_planned_bytes=7500000 max_nodes=98000 overruns=0 planning_ms=5"
        ));
    assert!(markdown.contains(
            "symbol_index: stream_ms=60 stream_rows=8192 stream_batches=2 docs=8192 writers=1 commits=1 commit_ms=64 reloads=1 reload_ms=65"
        ));
    assert!(markdown.contains(
            "full_refresh_wall_ms: core_refresh=1000 live_inspection=10 source_discovery=20 stage_open=30 indexer_execution=400 coverage_validation=40 copy_forward=50 semantic_stage=150 snapshot_stage=100 publication_prepare=50 search_generation=100 catalog_publication=30 unattributed=20"
        ));
    assert!(markdown.contains("indexer_io_ms: source_prepare=41 projection_batch_wall=50"));
    assert!(markdown.contains("projection_batches: transactions=2"));
    assert!(markdown.contains(
            "projection_persistence: transactions=2 rows=40 bound_bytes=4096 statements=35 transaction_wall_ms=48 setup_ms=1 commit_ms=7"
        ));
    assert!(
        markdown.contains(
            "projection_persistence.files: rows=4 bound_bytes=512 statements=4 wall_ms=3"
        )
    );
    assert!(markdown.contains(
        "projection_persistence.dirty_state: rows=8 bound_bytes=96 statements=8 wall_ms=2"
    ));
    assert!(
            markdown
                .contains("semantic_ms: context_index=59 node_load=66 endpoint_load=6 context=67 doc_build=7 embedding=8 db_upsert=9 reload=10 prune=64")
        );
    assert!(markdown.contains(
            "semantic_context: node_rows=8192 node_batches=2 endpoint_rows=4096 endpoint_batches=21 selected_nodes=2048 files=128 path_bytes=16384 lookup_peak=8192"
        ));
    assert!(markdown.contains("semantic_docs: reused=11 embedded=12 pending=13 stale=14"));
    assert!(markdown.contains(
        "staged_publish_ms: deferred_indexes=7 summary_snapshot=8 detail_snapshot=9 publish=10"
    ));
    assert!(
        markdown.contains(
            "staged_sqlite: wal_autocheckpoint_bytes=67108864 checkpoint_ms=11 sync_ms=12"
        )
    );
    assert!(
        markdown.contains("staged_snapshot_copy: copy_ms=13 source_bytes=1024 target_bytes=1024")
    );
    assert!(markdown.contains(
            "core_promotion_ms: total=89 lock_recovery=1 candidate_validation=11 previous_validation=12 rollback_backup_copy=13 backup_validation=14 prepared_journal_write=2 prepared_journal_file_sync=3 prepared_journal_directory_sync=4 staged_to_live_restore=15 promoted_validation=10 committed_journal=2 cleanup=1 unattributed=1"
        ));
    assert!(
        markdown.contains(
            "core_promotion_bytes: candidate=2048 previous_live=1024 rollback_backup=1024"
        )
    );
    assert!(markdown.contains("setup_ms: existing_projection_ids=11 seed_symbol_table=12"));
    assert!(
            markdown.contains(
                "flush_breakdown_ms: files=13 nodes=14 edges=15 occurrences=16 component_access=17 callable_projection=18"
            )
        );
    assert!(markdown.contains(
        "resolution_ms: override_count=25 unresolved_counts=26 calls=27 imports=28 cleanup=29"
    ));
    assert!(markdown.contains(
            "resolution_indexes_ms: call_candidate=30 import_candidate=31 call_semantic=32 import_semantic=33"
        ));
    assert!(markdown.contains(
        "resolution_support_snapshot: limit_bytes=1000000000 stored=true skipped_oversize=false"
    ));
    assert!(markdown.contains(
            "resolution_detail_ms: call_semantic_candidates=34 import_semantic_candidates=35 call_compute=42 import_compute=43 call_apply=44 import_apply=45 overrides=46"
        ));
    assert!(markdown.contains(
            "resolution_semantic_requests: call_rows=36 call_unique=37 call_skipped=38 import_rows=39 import_unique=40 import_skipped=41"
        ));
}
