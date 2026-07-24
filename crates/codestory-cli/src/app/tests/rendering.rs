use super::test_support::{
    sample_agent_answer_with_graph, sample_phase_timings, sample_retrieval,
    sample_task_brief_packet, summary_with_files, test_search_hit_defaults,
};
use crate::app::artifacts::{CONTEXT_BUNDLE_OUTPUT_BYTE_CAP, write_context_bundle};
use crate::app::drill::drill_search_hit_from_packet_citation;
use crate::app::rendering::{
    RepoTextOutputConfig, SearchOutputParts, build_query_resolution_output, build_search_output,
};
use crate::app::resolution::quote_command_value;
use crate::app::source_commands::append_symbol_workflow_nodes;
use crate::app::{build_task_brief_output, render_task_brief_markdown};
use crate::args::{Cli, IndexDryRunOutput, IndexOutput, QuerySelectorOutput, RepoTextMode};
use crate::explore::{ExploreTuiAction, ExploreTuiState, explore_tui_action};
use crate::http_transport::search_repo_text_mode_param;
use crate::output::{render_index_dry_run_markdown, render_index_markdown, render_search_markdown};
use crate::runtime::{self, fnv1a_hex};
use clap::Parser;
use codestory_contracts::api::{
    AgentCitationDto, GraphArtifactDto, IndexDryRunDto, IndexMode, NodeId, NodeKind, SearchHit,
    SearchRepoTextMode, SourceOccurrenceDto,
};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

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

#[test]
fn build_search_output_preserves_separate_provenance_groups() {
    let root = Path::new("C:/repo");
    let symbol_hits = vec![SearchHit {
        node_id: NodeId("1".to_string()),
        display_name: "indexed_symbol".to_string(),
        kind: codestory_contracts::api::NodeKind::FUNCTION,
        file_path: Some("src/lib.rs".to_string()),
        line: Some(10),
        score: 0.9,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: None,
        ..test_search_hit_defaults()
    }];
    let repo_text_hits = vec![SearchHit {
        node_id: NodeId("repo-text".to_string()),
        display_name: "README.md".to_string(),
        kind: codestory_contracts::api::NodeKind::FILE,
        file_path: Some("README.md".to_string()),
        line: Some(3),
        score: 500.0,
        origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
        match_quality: Some(codestory_contracts::api::SearchMatchQualityDto::RepoText),
        resolvable: false,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: None,
        ..test_search_hit_defaults()
    }];

    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "needle",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &symbol_hits,
        repo_text_hits: &repo_text_hits,
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &HashMap::new(),
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::Auto,
            enabled: true,
        },
        explain: false,
    });

    assert_eq!(output.repo_text_mode, RepoTextMode::Auto);
    assert!(output.repo_text_enabled);
    assert_eq!(output.indexed_symbol_hits.len(), 1);
    assert_eq!(output.repo_text_hits.len(), 1);
    assert_eq!(output.indexed_symbol_hits[0].display_name, "indexed_symbol");
    assert_eq!(output.repo_text_hits[0].display_name, "README.md");
    assert_eq!(
        output.repo_text_hits[0].origin,
        codestory_contracts::api::SearchHitOrigin::TextMatch
    );
}

#[test]
fn cli_search_and_resolution_keep_structural_evidence_metadata() {
    let root = Path::new("C:/repo");
    let manifest = SearchHit {
        node_id: NodeId("cargo-package".to_string()),
        display_name: "demo".to_string(),
        kind: NodeKind::PACKAGE,
        file_path: Some("C:/repo/Cargo.toml".to_string()),
        line: Some(2),
        score: 0.9,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        resolvable: true,
        evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText),
        evidence_producer: Some("structural_cargo_manifest_collector".to_string()),
        resolution_status: Some(
            codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly,
        ),
        eligible_for_sufficiency: Some(false),
        ..test_search_hit_defaults()
    };
    let workflow = SearchHit {
        node_id: NodeId("workflow-job".to_string()),
        display_name: "test".to_string(),
        kind: NodeKind::FUNCTION,
        file_path: Some("C:/repo/.github/workflows/ci.yml".to_string()),
        line: Some(12),
        score: 0.8,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        resolvable: true,
        evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText),
        evidence_producer: Some("structural_github_actions_workflow_collector".to_string()),
        resolution_status: Some(
            codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly,
        ),
        eligible_for_sufficiency: Some(false),
        ..test_search_hit_defaults()
    };
    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "demo",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &[manifest.clone(), workflow.clone()],
        repo_text_hits: &[],
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &HashMap::new(),
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::Off,
            enabled: false,
        },
        explain: false,
    });

    let search_json = serde_json::to_value(&output).expect("serialize CLI search output");
    for (index, producer) in [
        (0, "structural_cargo_manifest_collector"),
        (1, "structural_github_actions_workflow_collector"),
    ] {
        let hit = &search_json["indexed_symbol_hits"][index];
        assert_eq!(hit["evidence_tier"], "structural_text");
        assert_eq!(hit["evidence_producer"], producer);
        assert_eq!(hit["resolution_status"], "source_range_only");
        assert_eq!(hit["eligible_for_sufficiency"], false);
    }
    let markdown = render_search_markdown(root, &output);
    assert!(
        markdown.contains("evidence_tier=structural_text"),
        "{markdown}"
    );
    assert!(
        markdown.contains("resolution_status=source_range_only"),
        "{markdown}"
    );
    assert!(
        markdown.contains("eligible_for_sufficiency=false"),
        "{markdown}"
    );

    let target = runtime::ResolvedTarget {
        selector: QuerySelectorOutput::Query,
        requested: "demo".to_string(),
        file_filter: None,
        selected: manifest.clone(),
        alternatives: vec![manifest, workflow.clone()],
    };
    let resolution = build_query_resolution_output(root, &target);
    let resolution_json =
        serde_json::to_value(&resolution).expect("serialize CLI query resolution output");
    assert_eq!(
        resolution_json["resolved"]["evidence_tier"],
        "structural_text"
    );
    assert_eq!(
        resolution_json["resolved"]["resolution_status"],
        "source_range_only"
    );
    assert_eq!(
        resolution_json["resolved"]["eligible_for_sufficiency"],
        false
    );
    assert_eq!(
        resolution_json["alternatives"][0]["evidence_producer"],
        "structural_github_actions_workflow_collector"
    );

    let citation = AgentCitationDto {
        node_id: workflow.node_id.clone(),
        display_name: workflow.display_name.clone(),
        kind: workflow.kind,
        file_path: workflow.file_path.clone(),
        line: workflow.line,
        score: workflow.score,
        origin: workflow.origin,
        resolvable: workflow.resolvable,
        subgraph_id: None,
        evidence_edge_ids: Vec::new(),
        retrieval_score_breakdown: None,
        evidence_tier: workflow.evidence_tier,
        evidence_producer: workflow.evidence_producer.clone(),
        resolution_status: workflow.resolution_status,
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: workflow.eligible_for_sufficiency,
    };
    let drill_hit = drill_search_hit_from_packet_citation(root, "test", &citation);
    assert_eq!(
        drill_hit.evidence_producer.as_deref(),
        Some("structural_github_actions_workflow_collector")
    );
    assert_eq!(
        drill_hit.resolution_status,
        Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
    );
    assert_eq!(drill_hit.eligible_for_sufficiency, Some(false));
}

#[test]
fn build_search_output_marks_repo_text_why_as_diagnostic_navigation() {
    let root = Path::new("C:/repo");
    let repo_text_hits = vec![SearchHit {
        node_id: NodeId("repo-text".to_string()),
        display_name: "README.md".to_string(),
        kind: codestory_contracts::api::NodeKind::FILE,
        file_path: Some("README.md".to_string()),
        line: Some(3),
        score: 500.0,
        origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
        match_quality: Some(codestory_contracts::api::SearchMatchQualityDto::RepoText),
        resolvable: false,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: None,
        ..test_search_hit_defaults()
    }];

    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "needle",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &[],
        repo_text_hits: &repo_text_hits,
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &HashMap::new(),
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::Auto,
            enabled: true,
        },
        explain: true,
    });

    let why = output.repo_text_hits[0].why.join("\n");
    assert!(
        why.contains("repo-text diagnostic match"),
        "repo-text why should be a diagnostic/navigation hint: {why}"
    );
    assert!(
        !why.contains("this hit is evidence"),
        "repo-text why must not present text as evidence: {why}"
    );
}

#[test]
fn write_context_bundle_caps_disk_artifacts_and_writes_manifest() {
    let temp = tempdir().expect("bundle dir");
    fs::write(
        temp.path().join("big-mermaid.mmd"),
        "stale oversized artifact",
    )
    .expect("write stale artifact");
    fs::write(
        temp.path().join("previously-omitted.mmd"),
        "stale upstream-omitted artifact",
    )
    .expect("write stale upstream-omitted artifact");
    let answer = sample_agent_answer_with_graph(GraphArtifactDto::Mermaid {
        id: "big-mermaid".to_string(),
        title: "Big Mermaid".to_string(),
        diagram: "graph TD".to_string(),
        mermaid_syntax: format!(
            "graph TD\nA[{}]\n",
            "x".repeat(CONTEXT_BUNDLE_OUTPUT_BYTE_CAP + 1024)
        ),
    });
    let output = serde_json::json!({
        "_meta": {
            "codestory_publication": {
                "served_from": "complete_publication",
                "operation": {"operation_id": "public-context", "attempt": 1}
            }
        },
        "target": {"selector": "id", "requested": "big-mermaid"},
        "context": crate::output::context_packet_json(&answer),
    });

    write_context_bundle(temp.path(), &output, &answer.graphs, "short context")
        .expect("write capped bundle");

    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(temp.path().join("bundle_manifest.json"))
            .expect("read bundle manifest"),
    )
    .expect("parse bundle manifest");
    let context_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(temp.path().join("context.json")).expect("read context json"),
    )
    .expect("parse context json");

    assert_eq!(manifest["truncated"], serde_json::Value::Bool(true));
    assert_eq!(
        manifest["omitted_mermaid_artifacts"].as_u64(),
        Some(1),
        "{manifest}"
    );
    assert!(
        manifest["written_bytes_excluding_manifest"]
            .as_u64()
            .is_some_and(|bytes| bytes <= CONTEXT_BUNDLE_OUTPUT_BYTE_CAP as u64),
        "{manifest}"
    );
    assert_eq!(context_json["truncated"], serde_json::Value::Bool(true));
    assert_eq!(
        context_json.pointer("/_meta/codestory_publication/operation/operation_id"),
        Some(&serde_json::json!("public-context"))
    );
    assert!(
        !temp.path().join("big-mermaid.mmd").exists(),
        "oversized Mermaid artifact should be omitted"
    );
    assert!(
        !temp.path().join("previously-omitted.mmd").exists(),
        "stale Mermaid artifacts from prior runs should be removed"
    );
}

#[test]
fn http_search_repo_text_param_accepts_cli_modes() {
    assert_eq!(
        search_repo_text_mode_param("auto"),
        Some(SearchRepoTextMode::Auto)
    );
    assert_eq!(
        search_repo_text_mode_param("off"),
        Some(SearchRepoTextMode::Off)
    );
    assert_eq!(
        search_repo_text_mode_param("0"),
        Some(SearchRepoTextMode::Off)
    );
    assert_eq!(
        search_repo_text_mode_param("on"),
        Some(SearchRepoTextMode::On)
    );
    assert_eq!(search_repo_text_mode_param("bogus"), None);
}

#[test]
fn build_search_output_adds_stable_node_ref_when_location_is_known() {
    let root = Path::new("C:/repo");
    let symbol_hits = vec![SearchHit {
        node_id: NodeId("1".to_string()),
        display_name: "ResolutionPass".to_string(),
        kind: codestory_contracts::api::NodeKind::STRUCT,
        file_path: Some("C:/repo/src/resolution/mod.rs".to_string()),
        line: Some(42),
        score: 0.9,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: None,
        ..test_search_hit_defaults()
    }];

    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "ResolutionPass",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &symbol_hits,
        repo_text_hits: &[],
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &HashMap::new(),
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::Auto,
            enabled: false,
        },
        explain: false,
    });

    assert_eq!(
        output.indexed_symbol_hits[0].node_ref.as_deref(),
        Some("src/resolution/mod.rs:42:ResolutionPass")
    );
}

#[test]
fn build_search_output_adds_occurrence_quality_and_verification_targets() {
    let root = Path::new("C:/repo");
    let symbol_hits = vec![SearchHit {
        node_id: NodeId("1".to_string()),
        display_name: "StorageAccess".to_string(),
        kind: codestory_contracts::api::NodeKind::CLASS,
        file_path: Some("C:/repo/src/lib/StorageAccess.h".to_string()),
        line: Some(12),
        score: 0.9,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: None,
        ..test_search_hit_defaults()
    }];
    let mut occurrences = HashMap::new();
    occurrences.insert(
        NodeId("1".to_string()),
        vec![
            SourceOccurrenceDto {
                element_id: "1".to_string(),
                kind: "declaration".to_string(),
                file_path: "C:/repo/src/lib/StorageAccess.h".to_string(),
                start_line: 12,
                start_col: 1,
                end_line: 12,
                end_col: 20,
            },
            SourceOccurrenceDto {
                element_id: "1".to_string(),
                kind: "definition".to_string(),
                file_path: "C:/repo/src/lib/StorageAccess.cpp".to_string(),
                start_line: 44,
                start_col: 1,
                end_line: 60,
                end_col: 1,
            },
        ],
    );

    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "StorageAccess",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &symbol_hits,
        repo_text_hits: &[],
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &occurrences,
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::Auto,
            enabled: false,
        },
        explain: false,
    });

    let hit = &output.indexed_symbol_hits[0];
    assert_eq!(hit.primary_occurrence_kind.as_deref(), Some("definition"));
    assert_eq!(hit.symbol_role.as_deref(), Some("definition"));
    assert!(hit.verification_targets.iter().any(|target| target.path
            == "src/lib/StorageAccess.cpp"
            && target.role == "definition"));
    assert!(
        hit.paired_refs.iter().any(|target| {
            target.path == "src/lib/StorageAccess.h" && target.role == "declaration"
        }),
        "definition hits should point back to the paired declaration"
    );
}

#[test]
fn renderer_uses_operation_bound_excerpt_after_source_mutation() {
    let temp = tempdir().expect("tempdir");
    let source = temp.path().join("src/Project.h");
    let implementation = temp.path().join("src/Project.cpp");
    fs::create_dir_all(source.parent().expect("source parent")).expect("create source parent");
    fs::write(&source, "class Project { void buildIndex(); };\n").expect("write indexed source");
    fs::write(
        &implementation,
        "#include \"Project.h\"\n\nvoid Project::buildIndex() {}\n",
    )
    .expect("write indexed implementation");
    let hit = SearchHit {
        node_id: NodeId("text-1".to_string()),
        display_name: "Project::buildIndex".to_string(),
        kind: NodeKind::FILE,
        file_path: Some(source.to_string_lossy().into_owned()),
        line: Some(1),
        score: 500.0,
        origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
        match_quality: Some(codestory_contracts::api::SearchMatchQualityDto::RepoText),
        resolvable: false,
        source_excerpt: Some("class Project { void buildIndex(); };".to_string()),
        verification_targets: vec![codestory_contracts::api::SearchVerificationTargetDto {
            role: "definition".to_string(),
            file_path: implementation.to_string_lossy().into_owned(),
            line: 3,
            display_name: "Project::buildIndex".to_string(),
            reason: "sibling implementation location for a C/C++ header hit".to_string(),
        }],
        ..test_search_hit_defaults()
    };

    fs::write(&source, "class Replacement {};\n").expect("mutate source after result");
    fs::write(&implementation, "void unrelated() {}\n")
        .expect("mutate implementation after result");
    let output = build_search_output(SearchOutputParts {
        project_root: temp.path(),
        query: "value",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &[],
        repo_text_hits: &[hit],
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &HashMap::new(),
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::On,
            enabled: true,
        },
        explain: false,
    });

    assert_eq!(
        output.repo_text_hits[0].excerpt.as_deref(),
        Some("class Project { void buildIndex(); };")
    );
    assert_eq!(
        output.repo_text_hits[0].verification_targets[0].path,
        "src/Project.cpp"
    );
    assert_eq!(
        output.repo_text_hits[0].verification_targets[0].line, 3,
        "rendering must not relocate a pinned target from newer source bytes"
    );
}

#[test]
fn build_search_output_marks_repo_text_duplicates_of_indexed_symbols() {
    let root = Path::new("C:/repo");
    let symbol_hits = vec![SearchHit {
        node_id: NodeId("symbol-1".to_string()),
        display_name: "build_snapshot_digest".to_string(),
        kind: codestory_contracts::api::NodeKind::FUNCTION,
        file_path: Some("C:/repo/src/lib.rs".to_string()),
        line: Some(7),
        score: 0.9,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: None,
        ..test_search_hit_defaults()
    }];
    let repo_text_hits = vec![SearchHit {
        node_id: NodeId("text-1".to_string()),
        display_name: "src/lib.rs".to_string(),
        kind: codestory_contracts::api::NodeKind::FILE,
        file_path: Some("C:/repo/src/lib.rs".to_string()),
        line: Some(7),
        score: 500.0,
        origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
        match_quality: Some(codestory_contracts::api::SearchMatchQualityDto::RepoText),
        resolvable: false,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: None,
        ..test_search_hit_defaults()
    }];

    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "snapshot digest",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &symbol_hits,
        repo_text_hits: &repo_text_hits,
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &HashMap::new(),
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::Auto,
            enabled: true,
        },
        explain: false,
    });

    assert_eq!(
        output.repo_text_hits[0].duplicate_of.as_deref(),
        Some("symbol-1")
    );
}

#[test]
fn task_brief_output_contract_maps_packet_evidence_to_owner_workflow() {
    let packet = sample_task_brief_packet();
    let brief = build_task_brief_output(Path::new("C:/repo"), &packet);

    assert_eq!(brief.task_brief_version, 1);
    assert_eq!(brief.status, "needs_attention");
    assert_eq!(brief.source_packet_id, "packet-task-brief");
    assert_eq!(brief.source_packet_sufficiency, "partial");
    assert_eq!(
        brief.first_files[0].path,
        "crates/codestory-cli/src/`main_$env:SECRET$('x').rs"
    );
    assert_eq!(
        brief.relevant_symbols[0].name,
        "run_`packet_$env:SECRET$('x')"
    );
    assert_eq!(
        brief.likely_tests[0].path,
        "crates/codestory-cli/tests/stdio`$env:SECRET$('x')_protocol_contracts.rs"
    );
    assert!(
        brief
            .impacted_surfaces
            .contains(&"crates/codestory-cli".to_string())
    );
    assert!(
        brief
            .risks_unknowns
            .contains(&"verify `changed` files after editing".to_string())
    );
    for expected in [
        "codestory-cli packet",
        "codestory-cli snippet",
        "codestory-cli trail",
        "codestory-cli affected",
    ] {
        assert!(
            brief
                .follow_up_codestory_commands
                .iter()
                .any(|command| command.contains(expected)),
            "brief should include {expected}: {brief:#?}"
        );
    }
    assert_eq!(brief.future_sections, ["scout", "where", "onboard"]);

    let packet_command = brief
        .follow_up_codestory_commands
        .iter()
        .find(|command| command.contains("codestory-cli packet"))
        .expect("packet follow-up command");
    assert!(
        packet_command.contains(&format!(
            "--question {}",
            quote_command_value(&packet.question)
        )),
        "packet follow-up should quote prompt safely: {packet_command}"
    );
    let snippet_command = brief
        .follow_up_codestory_commands
        .iter()
        .find(|command| command.contains("codestory-cli snippet"))
        .expect("snippet follow-up command");
    assert!(
        snippet_command.contains(&quote_command_value(&brief.first_files[0].path)),
        "snippet follow-up should quote path safely: {snippet_command}"
    );
    let trail_command = brief
        .follow_up_codestory_commands
        .iter()
        .find(|command| command.contains("codestory-cli trail"))
        .expect("trail follow-up command");
    assert!(
        trail_command.contains(&quote_command_value(&brief.relevant_symbols[0].name)),
        "trail follow-up should quote symbol safely: {trail_command}"
    );

    let json = serde_json::to_value(&brief).expect("brief should serialize");
    for key in [
        "task_brief_version",
        "prompt",
        "status",
        "first_files",
        "relevant_symbols",
        "likely_tests",
        "impacted_surfaces",
        "risks_unknowns",
        "follow_up_codestory_commands",
        "future_sections",
    ] {
        assert!(json.get(key).is_some(), "brief JSON should include {key}");
    }

    let markdown = render_task_brief_markdown(&brief);
    assert!(
        markdown.contains("prompt: `Add '$env:SECRET $(Get-ChildItem) 'literal' task brief`"),
        "brief markdown should replace prompt backticks inside inline code: {markdown}"
    );
    assert!(
        markdown.contains("`crates/codestory-cli/src/'main_$env:SECRET$('x').rs`"),
        "brief markdown should replace path backticks inside inline code: {markdown}"
    );
    assert!(
        markdown.contains("`run_'packet_$env:SECRET$('x')`"),
        "brief markdown should replace symbol backticks inside inline code: {markdown}"
    );
    assert!(
        markdown.contains("- verify 'changed' files after editing"),
        "brief markdown should replace risk backticks in bullets: {markdown}"
    );
    assert!(
        markdown.contains("- command:\n    codestory-cli packet"),
        "brief markdown should render commands as indented code blocks: {markdown}"
    );
    assert!(
        !markdown.contains("- `codestory-cli"),
        "brief markdown should not render follow-up commands as inline code: {markdown}"
    );
    assert!(
        !markdown.contains("```"),
        "brief markdown should not use fences that embedded backticks can split: {markdown}"
    );
    for heading in [
        "# Task Brief",
        "## First Files",
        "## Relevant Symbols",
        "## Likely Tests",
        "## Impacted Surfaces",
        "## Risks And Unknowns",
        "## Follow Up CodeStory Commands",
        "## Future Sections",
    ] {
        assert!(
            markdown.contains(heading),
            "brief markdown should include {heading}: {markdown}"
        );
    }
}

#[test]
fn all_existing_commands_accept_output_file() {
    let commands = [
        vec!["codestory-cli", "index", "--output-file", "out.md"],
        vec!["codestory-cli", "ground", "--output-file", "out.md"],
        vec![
            "codestory-cli",
            "search",
            "--query",
            "needle",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "symbol",
            "--query",
            "Foo",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "trail",
            "--query",
            "Foo",
            "--hide-speculative",
            "--format",
            "dot",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "snippet",
            "--query",
            "Foo",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "task",
            "brief",
            "--prompt",
            "Implement issue 507",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "query",
            "search(query: 'Foo') | limit(1)",
            "--output-file",
            "out.md",
        ],
        vec!["codestory-cli", "doctor", "--output-file", "out.md"],
        vec![
            "codestory-cli",
            "explore",
            "--query",
            "Foo",
            "--no-tui",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "bookmark",
            "add",
            "--id",
            "1",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "bookmark",
            "list",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "bookmark",
            "remove",
            "1",
            "--output-file",
            "out.md",
        ],
    ];

    for command in commands {
        Cli::try_parse_from(command).expect("command should parse --output-file");
    }
}

#[test]
fn explore_tui_keyboard_state_reaches_every_pane() {
    let mut state = ExploreTuiState::new(6);
    for expected in 1..6 {
        assert!(!state.apply(ExploreTuiAction::NextPane));
        assert_eq!(state.selected, expected);
    }
    assert!(!state.apply(ExploreTuiAction::NextPane));
    assert_eq!(state.selected, 0);

    assert!(!state.apply(ExploreTuiAction::PreviousPane));
    assert_eq!(state.selected, 5);
    assert!(!state.apply(ExploreTuiAction::ScrollDown(12)));
    assert_eq!(state.scroll[5], 12);
    assert!(!state.apply(ExploreTuiAction::ScrollUp(5)));
    assert_eq!(state.scroll[5], 7);
    assert!(!state.apply(ExploreTuiAction::Home));
    assert_eq!(state.scroll[5], 0);
    assert!(state.apply(ExploreTuiAction::Quit));
}

#[test]
fn explore_tui_key_mapping_covers_keyboard_only_controls() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    assert_eq!(
        explore_tui_action(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        ExploreTuiAction::NextPane
    );
    assert_eq!(
        explore_tui_action(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        ExploreTuiAction::PreviousPane
    );
    assert_eq!(
        explore_tui_action(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
        ExploreTuiAction::ScrollDown(1)
    );
    assert_eq!(
        explore_tui_action(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
        ExploreTuiAction::ScrollUp(10)
    );
    assert_eq!(
        explore_tui_action(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
        ExploreTuiAction::Quit
    );
    assert_eq!(
        explore_tui_action(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        ExploreTuiAction::Quit
    );
}

#[test]
fn build_search_output_includes_why_when_requested() {
    let root = Path::new("C:/repo");
    let symbol_hits = vec![SearchHit {
        node_id: NodeId("1".to_string()),
        display_name: "ranked_symbol".to_string(),
        kind: codestory_contracts::api::NodeKind::FUNCTION,
        file_path: Some("src/lib.rs".to_string()),
        line: Some(10),
        score: 0.9,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: Some(codestory_contracts::api::RetrievalScoreBreakdownDto {
            lexical: 0.7,
            semantic: 0.2,
            graph: 0.1,
            total: 0.9,
            tier_cap: None,
            boosts: Vec::new(),
            dampening: Vec::new(),
            final_rank_reason: None,
            provenance: Vec::new(),
        }),
        ..test_search_hit_defaults()
    }];

    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "ranked",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &symbol_hits,
        repo_text_hits: &[],
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &HashMap::new(),
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::Off,
            enabled: false,
        },
        explain: true,
    });

    assert!(output.explain);
    assert_eq!(
        output.indexed_symbol_hits[0]
            .score_breakdown
            .as_ref()
            .map(|score| score.total),
        Some(0.9)
    );
    assert!(
        output.indexed_symbol_hits[0]
            .why
            .iter()
            .any(|why| why.contains("lexical=0.700"))
    );
}
