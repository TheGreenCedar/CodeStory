#![cfg(feature = "test-support")]
//! A supported local project reaches full retrieval through the embedded store
//! and its pinned publication evidence, and fails closed before a retrieval
//! generation is published.

use codestory_contracts::graph::{Node, NodeId, NodeKind};
use codestory_retrieval::test_support::{
    publish_complete_core_fixture, publish_zero_dense_pinned_query_fixture,
};
use codestory_retrieval::{
    QueryRequest, RetrievalCache, RetrievalStageKind, SidecarProcessDefaults, SidecarProfile,
    SidecarRuntimeConfig, SidecarRuntimeDefaults, SidecarRuntimeOverrides, StageCompletionStatus,
    execute_retrieval_query_with_cache_for_runtime,
};
use codestory_store::{
    FileInfo, FileRole, IndexPublicationMode, IndexPublicationRecord, SearchSymbolProjection,
    Store, SymbolSearchDoc,
};
use std::path::Path;
use tempfile::TempDir;

fn runtime(project_root: &Path, cache_root: &Path) -> SidecarRuntimeConfig {
    SidecarRuntimeConfig::for_project_profile_with_process_defaults(
        Some(project_root),
        SidecarProfile::Local,
        None,
        &SidecarProcessDefaults::new(cache_root.to_path_buf(), SidecarRuntimeDefaults::default()),
        &SidecarRuntimeOverrides::default(),
    )
}

fn seed_fixture_graph(storage: &mut Store, project_root: &Path) -> NodeId {
    let file_node_id = 1_001_i64;
    storage
        .insert_file(&FileInfo {
            id: file_node_id,
            path: project_root.join("lib.rs"),
            language: "rust".to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 3,
            file_role: FileRole::Entrypoint,
        })
        .expect("insert file");
    storage
        .insert_nodes_batch(&[Node {
            id: NodeId(file_node_id),
            kind: NodeKind::FILE,
            serialized_name: "lib.rs".to_string(),
            start_line: Some(1),
            start_col: Some(0),
            end_line: Some(3),
            end_col: Some(0),
            ..Default::default()
        }])
        .expect("file node");
    let function = Node {
        id: NodeId(2_001),
        kind: NodeKind::FUNCTION,
        serialized_name: "extension_service".to_string(),
        qualified_name: Some("extension_service".to_string()),
        file_node_id: Some(NodeId(file_node_id)),
        start_line: Some(1),
        start_col: Some(0),
        end_line: Some(1),
        end_col: Some(30),
        ..Default::default()
    };
    storage
        .insert_nodes_batch(std::slice::from_ref(&function))
        .expect("function node");
    storage
        .upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
            node_id: function.id,
            display_name: function.serialized_name.clone(),
        }])
        .expect("search projection");
    // A symbol search doc keeps the graph identity visible through the lexical
    // shard without producing dense anchors, so the fixture publication stays
    // strictly zero-dense.
    storage
        .upsert_symbol_search_docs_batch(&[SymbolSearchDoc {
            node_id: function.id,
            file_node_id: Some(NodeId(file_node_id)),
            kind: NodeKind::FUNCTION,
            display_name: function.serialized_name.clone(),
            qualified_name: function.qualified_name.clone(),
            file_path: Some("lib.rs".into()),
            start_line: Some(1),
            doc_text: "symbol_kind: FUNCTION\nname: extension_service".into(),
            doc_version: 1,
            doc_hash: "extension-service-doc".into(),
            policy_version: codestory_retrieval::SEMANTIC_POLICY_VERSION.into(),
            source_provenance: "graph".into(),
            updated_at_epoch_ms: 1,
        }])
        .expect("symbol search doc");
    let publication = IndexPublicationRecord {
        generation: 1,
        generation_id: "11111111-1111-4111-8111-111111111111".into(),
        run_id: "core-run-1".into(),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: 1,
    };
    publish_complete_core_fixture(storage, project_root, &publication)
        .expect("complete core fixture");
    function.id
}

fn seeded_project() -> (TempDir, TempDir, TempDir, std::path::PathBuf) {
    let project = TempDir::new().expect("project");
    std::fs::write(
        project.path().join("lib.rs"),
        "pub fn extension_service() {}\n",
    )
    .expect("write source");
    let cache = TempDir::new().expect("cache");
    let database = TempDir::new().expect("database");
    let storage_path = database.path().join("codestory.db");
    {
        let mut storage = Store::open(&storage_path).expect("open store");
        seed_fixture_graph(&mut storage, project.path());
    }
    (project, cache, database, storage_path)
}

#[test]
fn published_generation_reaches_full_mode_and_returns_graph_backed_hits() {
    let (project, cache, _database, storage_path) = seeded_project();
    let runtime = runtime(project.path(), cache.path());

    publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
        .expect("publish strict zero-dense generation");

    let result = execute_retrieval_query_with_cache_for_runtime(
        QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "extension_service",
            budget_ms: Some(2_000),
            cancelled: None,
        },
        &mut RetrievalCache::new(),
        &runtime,
    )
    .expect("query embedded retrieval");

    assert_eq!(result.trace.retrieval_mode, "full");
    assert!(
        result.trace.degraded_reason.is_none(),
        "full mode must not carry a degraded reason: {:?}",
        result.trace
    );
    assert!(
        result
            .hits
            .iter()
            .any(|hit| { hit.file_path == "lib.rs" && hit.node_id.as_deref() == Some("2001") }),
        "expected a graph-backed lexical hit, got {:?}",
        result.hits
    );
}

#[test]
fn natural_language_query_skips_semantic_stage_for_zero_dense_publication() {
    let (project, cache, _database, storage_path) = seeded_project();
    let runtime = runtime(project.path(), cache.path());

    publish_zero_dense_pinned_query_fixture(project.path(), &storage_path, &runtime)
        .expect("publish strict zero-dense generation");

    let result = execute_retrieval_query_with_cache_for_runtime(
        QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "how the extension service is implemented",
            budget_ms: Some(2_000),
            cancelled: None,
        },
        &mut RetrievalCache::new(),
        &runtime,
    )
    .expect("query embedded retrieval");

    assert_eq!(result.trace.retrieval_mode, "full");
    let semantic_stage = result
        .trace
        .stages
        .iter()
        .find(|stage| stage.stage == RetrievalStageKind::Stage1bSemantic)
        .expect("semantic stage trace");
    assert_eq!(
        semantic_stage.completion_status,
        StageCompletionStatus::Skipped,
        "zero-dense publications must skip the semantic stage instead of degrading"
    );
    assert_eq!(
        semantic_stage.cancel_reason.as_deref(),
        Some("zero_dense_anchors"),
        "the skip must be attributed to the zero-dense publication: {semantic_stage:?}"
    );
    assert!(
        !semantic_stage.degraded,
        "a planned zero-dense skip is not a degraded stage: {semantic_stage:?}"
    );
}

#[test]
fn query_fails_closed_before_a_retrieval_generation_is_published() {
    let (project, cache, _database, storage_path) = seeded_project();
    let runtime = runtime(project.path(), cache.path());

    let error = execute_retrieval_query_with_cache_for_runtime(
        QueryRequest {
            project_root: project.path(),
            storage_path: &storage_path,
            query: "extension_service",
            budget_ms: Some(2_000),
            cancelled: None,
        },
        &mut RetrievalCache::new(),
        &runtime,
    )
    .expect_err("queries must fail closed without a published retrieval generation");

    assert!(
        format!("{error:#}").contains("retrieval sidecar manifest is missing"),
        "fail-closed error should name the missing retrieval generation: {error:#}"
    );
}
